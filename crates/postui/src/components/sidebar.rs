use super::{Component, DrawCtx, pane_block};
use crate::action::Action;
use crate::hit::{self, Hit, HitMap, ScrollbarSpec};
use crate::layout::PaneId;
use crate::theme::Theme;
use postui_core::storage::RequestListing;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::BTreeSet;

/// One visible row of the sidebar tree: either a collapsible folder (derived
/// from a slug prefix, selectable but never "open") or a request leaf
/// (selectable, possibly broken).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Folder {
        path: String,
        name: String,
        depth: usize,
        expanded: bool,
    },
    Request {
        slug: String,
        depth: usize,
        broken: Option<String>,
    },
}

/// Identifies a row across a `refresh` rebuild so the previous selection can
/// be relocated in the new tree even though row indices shift.
enum RowId {
    Folder(String),
    Request(String),
}

#[derive(Default)]
pub struct Sidebar {
    pub rows: Vec<Row>,
    /// Index into `rows`; meaningless (and never read) while `rows` is empty.
    pub selected: usize,
    /// Index of the first row drawn; `draw` keeps `selected` inside the
    /// visible window whenever `ensure_visible` is set, by adjusting this.
    pub scroll: usize,
    /// The full flat listing behind the current tree, kept so future
    /// rebuilds don't need a caller-supplied copy.
    listing: Vec<RequestListing>,
    /// Ancestor folder paths that `select_slug` needs opened to make its
    /// target visible. The caller (`App::refresh_sidebar`) merges these into
    /// `project.expanded` and clears this set on the next refresh.
    pub pending_expand: BTreeSet<String>,
    /// Set whenever the *selection* moves (`move_selection`, `select_slug`,
    /// `refresh`) so the next `draw` scrolls it into view. Wheel scrolling
    /// (`handle_scroll`) never sets this: it's free to move the viewport
    /// without dragging the selection along, and `draw` must not snap it
    /// back.
    pub ensure_visible: bool,
    /// Height of the row list as of the last draw — the scrollbar's viewport.
    last_list_height: usize,
    /// The slug currently loaded in the editor, kept in sync by
    /// `App::update` after every action.
    pub open_slug: Option<String>,
    /// Whether the open request has unsaved changes, likewise kept in sync
    /// by `App::update`.
    pub open_dirty: bool,
}

impl Sidebar {
    /// Rebuilds `rows` as a tree from a fresh listing: at each level, this
    /// level's requests come first (sorted by slug), then its subfolders
    /// (sorted by name); a folder's children are emitted only when
    /// `expanded` contains its path. Preserves the current selection by row
    /// identity (folder path or request slug) across the rebuild, else
    /// clamps to the first row.
    pub fn refresh(&mut self, listing: Vec<RequestListing>, expanded: &BTreeSet<String>) {
        let prev = self.selected_identity();

        self.listing = listing;
        let mut sorted = self.listing.clone();
        sorted.sort_by(|a, b| a.slug.cmp(&b.slug));

        let mut rows = Vec::new();
        Self::build_rows(&sorted, "", 0, expanded, &mut rows);
        self.rows = rows;

        self.selected = prev
            .and_then(|id| self.rows.iter().position(|r| Self::row_matches(r, &id)))
            .unwrap_or(0);
        self.ensure_visible = true;
    }

    /// Builds the rows for one folder level (`prefix`, possibly empty for
    /// the root): direct requests first, in slug order, then subfolders in
    /// name order, recursing into each expanded one. `entries` must already
    /// be sorted by slug and every entry must start with `prefix`.
    fn build_rows(
        entries: &[RequestListing],
        prefix: &str,
        depth: usize,
        expanded: &BTreeSet<String>,
        rows: &mut Vec<Row>,
    ) {
        let mut folder_children: std::collections::BTreeMap<String, Vec<RequestListing>> =
            std::collections::BTreeMap::new();
        for e in entries {
            let rest = &e.slug[prefix.len()..];
            if let Some((seg, _)) = rest.split_once('/') {
                let path = if prefix.is_empty() {
                    seg.to_string()
                } else {
                    format!("{prefix}{seg}")
                };
                folder_children.entry(path).or_default().push(e.clone());
            } else {
                rows.push(Row::Request {
                    slug: e.slug.clone(),
                    depth,
                    broken: e.broken.clone(),
                });
            }
        }
        for (path, children) in folder_children {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let is_expanded = expanded.contains(&path);
            rows.push(Row::Folder {
                path: path.clone(),
                name,
                depth,
                expanded: is_expanded,
            });
            if is_expanded {
                let child_prefix = format!("{path}/");
                Self::build_rows(&children, &child_prefix, depth + 1, expanded, rows);
            }
        }
    }

    fn selected_identity(&self) -> Option<RowId> {
        match self.rows.get(self.selected)? {
            Row::Folder { path, .. } => Some(RowId::Folder(path.clone())),
            Row::Request { slug, .. } => Some(RowId::Request(slug.clone())),
        }
    }

    fn row_matches(row: &Row, id: &RowId) -> bool {
        match (row, id) {
            (Row::Folder { path, .. }, RowId::Folder(p)) => path == p,
            (Row::Request { slug, .. }, RowId::Request(s)) => slug == s,
            _ => false,
        }
    }

    /// Selects the request row for `slug`, if one is currently visible, and
    /// records `slug`'s ancestor folder paths in `pending_expand` so the
    /// caller can open them before the next `refresh` (which is what
    /// actually makes the row visible when it wasn't already).
    pub fn select_slug(&mut self, slug: &str) {
        let mut parts: Vec<&str> = slug.split('/').collect();
        parts.pop(); // drop the request's own basename; the rest are ancestor folders
        let mut acc = String::new();
        for seg in parts {
            if acc.is_empty() {
                acc = seg.to_string();
            } else {
                acc = format!("{acc}/{seg}");
            }
            self.pending_expand.insert(acc.clone());
        }

        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug: s, .. } if s == slug))
        {
            self.selected = i;
            self.ensure_visible = true;
        }
    }

    /// The slug of the currently selected request row, or `None` if the
    /// sidebar is empty or the selection is on a `Folder` row.
    pub fn selected_slug(&self) -> Option<String> {
        match self.rows.get(self.selected) {
            Some(Row::Request { slug, .. }) => Some(slug.clone()),
            _ => None,
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Reports the currently selected folder's path and the state it should
    /// flip to (`!expanded`), without changing anything itself: the caller
    /// owns the expanded set and is expected to update it, refresh, and
    /// reselect the folder (which happens automatically, by identity, on
    /// the next `refresh`).
    pub fn toggle_selected_folder(&mut self) -> Option<(String, bool)> {
        match self.selected_row()? {
            Row::Folder { path, expanded, .. } => Some((path.clone(), !expanded)),
            Row::Request { .. } => None,
        }
    }

    /// Moves `selected` by `delta` rows over the full tree (folders
    /// included), clamped to the first/last row.
    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let new = (self.selected as i32 + delta).clamp(0, self.rows.len() as i32 - 1) as usize;
        self.selected = new;
        self.ensure_visible = true;
    }

    /// The path of the parent folder that would contain `row`, if any.
    fn parent_path_of(row: &Row) -> Option<String> {
        match row {
            Row::Folder { path, .. } => path.rsplit_once('/').map(|(p, _)| p.to_string()),
            Row::Request { slug, .. } => slug.rsplit_once('/').map(|(p, _)| p.to_string()),
        }
    }

    /// Moves the selection to the parent folder row of the current
    /// selection, if it has one and that row is visible. A no-op (still
    /// redraws) when there's no parent, e.g. at the top level.
    fn jump_to_parent(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(parent) = Self::parent_path_of(row) else {
            return;
        };
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Folder { path, .. } if *path == parent))
        {
            self.selected = i;
            self.ensure_visible = true;
        }
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
                Row::Request {
                    slug, broken: None, ..
                } => Some(Action::OpenRequest(slug.clone())),
                Row::Request {
                    slug,
                    broken: Some(_),
                    ..
                } => Some(Action::ShowRequestError(slug.clone())),
                Row::Folder { .. } => Some(Action::ToggleSelectedFolder),
            },
            KeyCode::Right => matches!(
                self.selected_row()?,
                Row::Folder {
                    expanded: false,
                    ..
                }
            )
            .then_some(Action::ToggleSelectedFolder),
            KeyCode::Left => match self.selected_row()? {
                Row::Folder { expanded: true, .. } => Some(Action::ToggleSelectedFolder),
                _ => {
                    self.jump_to_parent();
                    Some(Action::Render)
                }
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
        // An explicit wheel gesture takes viewport control and cancels any
        // snap-into-view still pending from an earlier selection change.
        self.ensure_visible = false;
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, hits: &mut HitMap) {
        let block = pane_block("Requests", ctx);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let button_height = inner.height.min(1);
        let button_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: button_height,
        };
        hit::button(
            frame,
            hits,
            button_area,
            "+ New request",
            Hit::SidebarNewRequest,
            ctx.hovered,
            true,
            ctx.theme,
        );

        // One spacer line below the button; the row list starts on the
        // third line and shrinks the usable height by 2 accordingly.
        let list_y = inner.y.saturating_add(2).min(inner.y + inner.height);
        let mut list_area = Rect {
            x: inner.x,
            y: list_y,
            width: inner.width,
            height: inner.height.saturating_sub(2),
        };
        self.last_list_height = list_area.height as usize;

        if self.rows.is_empty() {
            let empty = Paragraph::new(vec![Line::raw(""), Line::raw("No requests yet.")])
                .style(Style::default().fg(ctx.theme.text_muted))
                .centered();
            frame.render_widget(empty, list_area);
            return;
        }

        let visible_height = list_area.height as usize;
        if self.ensure_visible {
            if visible_height > 0 {
                if self.selected < self.scroll {
                    self.scroll = self.selected;
                } else if self.selected >= self.scroll + visible_height {
                    self.scroll = self.selected + 1 - visible_height;
                }
                let max_scroll = self.rows.len().saturating_sub(visible_height);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        // Drawn after the scroll offset has settled (so the thumb is never a
        // frame behind) and before the rows, which give up the last column to
        // it so no row text ever runs underneath the bar.
        if let Some(spec) = self.scrollbar_spec().filter(ScrollbarSpec::overflows)
            && list_area.width > 1
        {
            let column = Rect {
                x: list_area.x + list_area.width - 1,
                width: 1,
                ..list_area
            };
            list_area.width -= 1;
            hit::draw_scrollbar(
                frame,
                hits,
                column,
                &spec,
                ctx.hovered,
                ctx.dragging,
                ctx.theme,
            );
        }

        for (display_pos, (i, row)) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(visible_height.max(1))
            .enumerate()
        {
            let row_rect = Rect {
                x: list_area.x,
                y: list_area.y + display_pos as u16,
                width: list_area.width,
                height: 1,
            };
            let line = self.render_row(i, row, ctx);
            let mut para = Paragraph::new(line);
            if ctx.hovered == Some(&Hit::SidebarRow(i)) {
                para = para.style(Style::default().bg(ctx.theme.surface_raised));
            }
            frame.render_widget(para, row_rect);
            hits.register(row_rect, Hit::SidebarRow(i));

            if let Row::Folder { depth, .. } = row {
                let arrow_x = row_rect.x + 2 + (*depth as u16) * 2;
                if arrow_x < row_rect.x + row_rect.width {
                    let arrow_rect = Rect {
                        x: arrow_x,
                        y: row_rect.y,
                        width: 1,
                        height: 1,
                    };
                    hits.register(arrow_rect, Hit::SidebarFolderArrow(i));
                }
            }
        }
    }
}

impl Sidebar {
    /// The row list's scroll state, as of the last draw. `None` before the
    /// first frame (the viewport height is a render-time fact).
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        if self.last_list_height == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: self.scroll,
            content: self.rows.len(),
            viewport: self.last_list_height,
        })
    }

    fn render_row(&self, idx: usize, row: &Row, ctx: &DrawCtx) -> Line<'static> {
        let theme: &Theme = ctx.theme;
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
        let marker = if is_selected { "\u{203a} " } else { "  " };

        match row {
            Row::Folder {
                name,
                depth,
                expanded,
                ..
            } => {
                let text_style = if is_selected && ctx.focused {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text_muted)
                };
                let glyph = if *expanded { "\u{25be}" } else { "\u{25b8}" };
                let indent = "  ".repeat(*depth);
                Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::raw(indent),
                    Span::styled(format!("{glyph} {name}/"), text_style),
                ])
            }
            Row::Request {
                slug,
                depth,
                broken,
            } => {
                let text_style = if is_selected && ctx.focused {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                let basename = slug.rsplit('/').next().unwrap_or(slug.as_str());
                let indent = "  ".repeat(*depth);

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
    use crate::components::DrawCtx;
    use crate::theme::Theme;
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

    fn expanded(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tree_builds_nested_folders_and_hides_collapsed_children() {
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["api/users/list", "api/users/create", "api/ping", "top"]),
            &expanded(&[]),
        );
        // collapsed: only top-level rows visible
        assert_eq!(
            s.rows,
            vec![
                Row::Request {
                    slug: "top".into(),
                    depth: 0,
                    broken: None
                },
                Row::Folder {
                    path: "api".into(),
                    name: "api".into(),
                    depth: 0,
                    expanded: false
                },
            ]
        );
        s.refresh(
            listing(&["api/users/list", "api/users/create", "api/ping", "top"]),
            &expanded(&["api"]),
        );
        assert_eq!(
            s.rows,
            vec![
                Row::Request {
                    slug: "top".into(),
                    depth: 0,
                    broken: None
                },
                Row::Folder {
                    path: "api".into(),
                    name: "api".into(),
                    depth: 0,
                    expanded: true
                },
                Row::Request {
                    slug: "api/ping".into(),
                    depth: 1,
                    broken: None
                },
                Row::Folder {
                    path: "api/users".into(),
                    name: "users".into(),
                    depth: 1,
                    expanded: false
                },
            ]
        );
    }

    #[test]
    fn enter_and_arrows_toggle_folders_and_navigate_all_rows() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["api/ping", "top"]), &expanded(&[]));
        s.handle_key(key(KeyCode::Char('j'))); // now on the "api" folder row
        assert!(matches!(s.rows[s.selected], Row::Folder { .. }));
        assert_eq!(s.selected_slug(), None, "folder rows have no slug");
        // Enter on a folder emits ToggleSelectedFolder
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(Action::ToggleSelectedFolder)
        );
    }

    #[test]
    fn wheel_scroll_is_free_and_keyboard_still_tracks_selection() {
        let mut s = Sidebar::default();
        let slugs: Vec<String> = (0..30).map(|i| format!("r{i:02}")).collect();
        let refs: Vec<&str> = slugs.iter().map(|s| s.as_str()).collect();
        s.refresh(listing(&refs), &expanded(&[]));
        s.handle_scroll(10);
        assert_eq!(s.scroll, 10);
        // drawing must NOT snap back to the selection
        let theme = Theme::dark();
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
        };
        let backend = ratatui::backend::TestBackend::new(30, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        assert_eq!(s.scroll, 10, "free scroll survives draw");
        // moving the selection scrolls it back into view
        s.handle_key(key(KeyCode::Char('j')));
        let mut hits = crate::hit::HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        assert!(
            s.scroll <= 1,
            "keyboard nav brings the selection into view: {}",
            s.scroll
        );
    }

    #[test]
    fn select_slug_expands_ancestor_folders() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a/b/c"]), &expanded(&[]));
        s.select_slug("a/b/c");
        assert!(s.pending_expand.contains("a") && s.pending_expand.contains("a/b"));
    }

    fn draw_ctx<'a>(theme: &'a Theme, hovered: Option<&'a Hit>) -> DrawCtx<'a> {
        DrawCtx {
            theme,
            focused: true,
            hovered,
            dragging: false,
        }
    }

    /// Renders `s` into a 30x12 terminal and hands back the buffer dump plus
    /// the frame's hits.
    fn render_hits(s: &mut Sidebar) -> (String, HitMap) {
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, None);
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        (format!("{:?}", terminal.backend().buffer()), hits)
    }

    #[test]
    fn overflowing_list_draws_a_scrollbar_and_registers_its_hits() {
        let slugs: Vec<String> = (0..30).map(|i| format!("r{i:02}")).collect();
        let refs: Vec<&str> = slugs.iter().map(String::as_str).collect();
        let mut s = Sidebar::default();
        s.refresh(listing(&refs), &expanded(&[]));

        let (content, hits) = render_hits(&mut s);
        assert!(content.contains('\u{2588}'), "thumb glyph is drawn");
        let thumb = hits
            .rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar))
            .expect("thumb hit");
        assert_eq!(thumb.width, 1);
        // 30 wide, minus a border and a padding column each side -> the bar
        // owns the last column of the padded interior.
        assert_eq!(thumb.x, 27);
        let track = hits.track_of(PaneId::Sidebar).expect("track rect");
        assert_eq!(track.x, thumb.x);
        // Inner height 10, minus the button row and its spacer.
        let viewport = 8i16;
        assert!(
            thumb.height < track.height,
            "30 rows in an 8-row viewport is a short thumb"
        );
        assert!(
            hits.rect_of(&Hit::ScrollbarTrack(PaneId::Sidebar, viewport))
                .is_some(),
            "page-down segment below the thumb"
        );
        assert!(
            hits.rect_of(&Hit::ScrollbarTrack(PaneId::Sidebar, -viewport))
                .is_none(),
            "no page-up segment while the thumb is at the top"
        );

        // Scrolled into the middle: both page segments exist.
        s.scroll = 10;
        s.ensure_visible = false;
        let (_, hits) = render_hits(&mut s);
        assert!(
            hits.rect_of(&Hit::ScrollbarTrack(PaneId::Sidebar, -viewport))
                .is_some(),
            "page-up segment above the thumb"
        );
        assert!(
            hits.rect_of(&Hit::ScrollbarTrack(PaneId::Sidebar, viewport))
                .is_some()
        );
        let thumb = hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar)).unwrap();
        assert!(thumb.y > track.y, "thumb moved down with the scroll offset");
    }

    #[test]
    fn short_list_registers_no_scrollbar() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), &expanded(&[]));
        let (content, hits) = render_hits(&mut s);
        assert!(!content.contains('\u{2588}'));
        assert_eq!(hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar)), None);
        assert_eq!(hits.track_of(PaneId::Sidebar), None);
        // The rows keep the full inner width when no bar is needed.
        let row = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        assert_eq!(row.width, 26);
    }

    #[test]
    fn draw_registers_new_request_button_row_hits_and_folder_arrow() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["api/ping", "top"]), &expanded(&["api"]));
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, None);
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("+ New request"));

        let button_rect = hits.rect_of(&Hit::SidebarNewRequest).expect("button hit");
        // Button is the first drawn line, at the top of the inner pane area.
        assert_eq!(
            button_rect.y, 1,
            "button sits on the first line inside the block"
        );

        // rows[0] = "top", rows[1] = folder "api" (expanded), rows[2] = "api/ping"
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).expect("row 0 hit");
        assert!(
            row0.y > button_rect.y + 1,
            "rows start after the button and a spacer line"
        );
        assert!(hits.rect_of(&Hit::SidebarRow(1)).is_some());
        assert!(hits.rect_of(&Hit::SidebarRow(2)).is_some());

        // Folder row 1 also registers its arrow glyph cell, which must win
        // over the row hit at that exact point (registered after it).
        let arrow_rect = hits
            .rect_of(&Hit::SidebarFolderArrow(1))
            .expect("folder arrow hit");
        assert_eq!(arrow_rect.width, 1);
        assert_eq!(
            hits.hit_at(arrow_rect.x, arrow_rect.y),
            Some(&Hit::SidebarFolderArrow(1)),
            "arrow hit wins over the row hit underneath it"
        );
    }

    #[test]
    fn hovered_row_gets_background_not_inverted_text() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["top"]), &expanded(&[]));
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, Some(&Hit::SidebarRow(0)));
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let cell = terminal.backend().buffer()[(row0.x, row0.y)].clone();
        assert_eq!(
            cell.bg, theme.surface_raised,
            "hovered row uses a raised background, not inverted fg/bg"
        );
    }
}
