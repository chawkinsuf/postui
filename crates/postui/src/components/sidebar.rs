use super::{Component, DrawCtx};
use crate::action::Action;
use crate::anim::{AnimKey, ListId};
use crate::hit::{self, Hit, HitMap, ScrollbarSpec};
use crate::layout::PaneId;
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, ListRow, RowHighlight, fill, frac_vspan, text,
};
use crate::theme::Theme;
use postui_core::model::Method;
use postui_core::storage::RequestListing;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
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
        /// The label the row paints: the request's display name, falling
        /// back to the slug leaf for legacy/broken files.
        name: String,
        depth: usize,
        broken: Option<String>,
        /// `None` exactly when `broken` is `Some` — a request whose file
        /// failed to parse has no method to show a chip for.
        method: Option<Method>,
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
    /// Index of the cursor/selected row, or `None` when no row is selected
    /// (a fresh sidebar with nothing open yet, or the previously selected
    /// row disappeared in a rebuild). The selected fill is honest: it only
    /// ever sits on a row the user actually put it on.
    pub selected: Option<usize>,
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
    /// level's requests come first (sorted by display name), then its
    /// subfolders (sorted by name); a folder's children are emitted only when
    /// `expanded` contains its path. Preserves the current selection by row
    /// identity (folder path or request slug) across the rebuild; a
    /// selection whose row vanished clears rather than sliding onto an
    /// arbitrary neighbor.
    pub fn refresh(&mut self, listing: Vec<RequestListing>, expanded: &BTreeSet<String>) {
        let prev = self.selected_identity();

        self.listing = listing;
        let mut sorted = self.listing.clone();
        sorted.sort_by(|a, b| a.slug.cmp(&b.slug));

        let mut rows = Vec::new();
        Self::build_rows(&sorted, "", 0, expanded, &mut rows);
        self.rows = rows;

        self.selected =
            prev.and_then(|id| self.rows.iter().position(|r| Self::row_matches(r, &id)));
        self.ensure_visible = true;
    }

    /// Builds the rows for one folder level (`prefix`, possibly empty for
    /// the root): direct requests first, in display-name order, then
    /// subfolders in name order, recursing into each expanded one.
    /// `entries` must already be sorted by slug (the folder grouping walks
    /// slug paths) and every entry must start with `prefix`.
    fn build_rows(
        entries: &[RequestListing],
        prefix: &str,
        depth: usize,
        expanded: &BTreeSet<String>,
        rows: &mut Vec<Row>,
    ) {
        let mut folder_children: std::collections::BTreeMap<String, Vec<RequestListing>> =
            std::collections::BTreeMap::new();
        let mut requests: Vec<&RequestListing> = Vec::new();
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
                requests.push(e);
            }
        }
        // This level's requests order by what the user reads — the display
        // name — with the slug as a stable tiebreak.
        requests.sort_by_key(|e| {
            let leaf = e.slug.rsplit('/').next().unwrap_or(&e.slug);
            let display = e.name.as_deref().unwrap_or(leaf);
            (display.to_lowercase(), e.slug.clone())
        });
        for e in requests {
            let leaf = e.slug.rsplit('/').next().unwrap_or(&e.slug);
            rows.push(Row::Request {
                name: e.name.clone().unwrap_or_else(|| leaf.to_string()),
                slug: e.slug.clone(),
                depth,
                broken: e.broken.clone(),
                method: e.method,
            });
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
        match self.rows.get(self.selected?)? {
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
            self.selected = Some(i);
            self.ensure_visible = true;
        }
    }

    /// The slug of the currently selected request row, or `None` if nothing
    /// is selected or the selection is on a `Folder` row.
    pub fn selected_slug(&self) -> Option<String> {
        match self.rows.get(self.selected?) {
            Some(Row::Request { slug, .. }) => Some(slug.clone()),
            _ => None,
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected?)
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
    /// included), clamped to the first/last row. With nothing selected the
    /// first press lands on the top row rather than being a dead key.
    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let new = match self.selected {
            Some(cur) => (cur as i32 + delta).clamp(0, self.rows.len() as i32 - 1) as usize,
            None => 0,
        };
        self.selected = Some(new);
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
            self.selected = Some(i);
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
        let theme = ctx.theme;
        let buf = frame.buffer_mut();
        fill(buf, area, theme.panel);

        if area.height == 0 || area.width == 0 {
            self.last_list_height = 0;
            return;
        }

        text(
            buf,
            area.x + 1,
            area.y,
            "REQUESTS",
            theme.text_muted,
            theme.panel,
            true,
        );

        let button_top = (area.y + 1).min(area.y + area.height);
        let button_height = BUTTON_HEIGHT.min(area.y + area.height - button_top);
        if button_height == BUTTON_HEIGHT {
            // Inset one column each side, like the other panes' content:
            // the left padding column belongs to the pane focus bar, and an
            // accent-filled button drawn into it would swallow the bar
            // (accent-on-accent) and read as bleeding flush into the edge.
            let button_area = Rect {
                x: area.x + 1,
                y: button_top,
                width: area.width.saturating_sub(2),
                height: button_height,
            };
            let state = if ctx.hovered == Some(&Hit::SidebarNewRequest) {
                ControlState::Hover
            } else {
                ControlState::Normal
            };
            Button {
                label: "+ New request",
                kind: ButtonKind::Primary,
                state,
            }
            .paint(buf, button_area, theme.panel, theme);
            hits.register(button_area, Hit::SidebarNewRequest);
        }

        // One blank spacer line below the button; the row list starts after it.
        // Rows share the button's 1-column inset each side, so column
        // `area.x` stays the pane focus bar's lane (no collision with the
        // selected row's accent marker) and the right margin column hosts
        // the scrollbar.
        let list_top = (button_top + button_height + 1).min(area.y + area.height);
        let list_area = Rect {
            x: area.x + 1,
            y: list_top,
            width: area.width.saturating_sub(2),
            height: (area.y + area.height) - list_top,
        };
        self.last_list_height = list_area.height as usize;

        if self.rows.is_empty() {
            let empty = Paragraph::new(vec![Line::raw(""), Line::raw("No requests yet.")])
                .style(Style::default().fg(theme.text_muted).bg(theme.panel));
            frame.render_widget(empty.centered(), list_area);
            return;
        }

        let visible_height = Self::visible_rows(list_area.height);
        if self.ensure_visible {
            if visible_height > 0
                && let Some(selected) = self.selected
            {
                if selected < self.scroll {
                    self.scroll = selected;
                } else if selected >= self.scroll + visible_height {
                    self.scroll = selected + 1 - visible_height;
                }
                let max_scroll = self.rows.len().saturating_sub(visible_height);
                self.scroll = self.scroll.min(max_scroll);
            }
            self.ensure_visible = false;
        }

        // Drawn after the scroll offset has settled (so the thumb is never a
        // frame behind). It lives in the pane's right margin column — the
        // one the inset list already leaves free — so rows keep their width.
        if let Some(spec) = self.scrollbar_spec().filter(ScrollbarSpec::overflows)
            && area.width > 1
        {
            let column = Rect {
                x: area.x + area.width - 1,
                width: 1,
                ..list_area
            };
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

        let zebra = self.zebra_parities();
        let hover_t = ctx.hover_t();

        // The keyboard cursor's travel band: `Some((y0, y1))` in *screen*
        // row-space, only while it's actually mid-flight (still easing
        // toward `self.selected`'s row). Once it arrives — or the anim was
        // never wired up at all, e.g. every sidebar-only unit test — this
        // is `None` and the loop below degenerates to painting the
        // selected row exactly like any other `ListRow::Selected`, with no
        // `frac_vspan` call at all.
        let travel_key = AnimKey::ListTravel(ListId::Sidebar);
        let cursor_settled_at = self.selected.filter(|_| ctx.focused);
        let anim_val = cursor_settled_at.map(|c| ctx.anims.value_or(travel_key, ctx.now, c as f32));
        let settled = match (cursor_settled_at, anim_val) {
            (Some(c), Some(v)) => (v - c as f32).abs() < 0.01,
            _ => true,
        };
        let band_range = if !settled {
            anim_val.map(|v| {
                let y0 = list_area.y as f32 + (v - self.scroll as f32);
                (y0, y0 + 1.0)
            })
        } else {
            None
        };

        let buf = frame.buffer_mut();

        if let Some((y0, y1)) = band_range {
            let clip_top = list_area.y as f32;
            let clip_bottom = (list_area.y + list_area.height) as f32;
            let cy0 = y0.max(clip_top);
            let cy1 = y1.min(clip_bottom);
            if cy1 > cy0 {
                // Accent bar column, then the rest of the row's width —
                // two spans so the bar keeps its own color through the
                // fractional edge frames instead of blending into the
                // fill's tint.
                frac_vspan(
                    buf,
                    list_area.x,
                    list_area.x + 1,
                    cy0,
                    cy1,
                    theme.accent,
                    theme.panel,
                );
                frac_vspan(
                    buf,
                    list_area.x + 1,
                    list_area.x + list_area.width,
                    cy0,
                    cy1,
                    theme.selection,
                    theme.panel,
                );
            }
        }

        for (display_pos, (i, row)) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(visible_height.max(1))
            .enumerate()
        {
            let text_row = list_area.y + display_pos as u16;
            if text_row >= area.y + area.height {
                break;
            }
            // Two separate things can mark a row, and they stay visually
            // distinct: the keyboard cursor (`self.selected`) gets the full
            // `Selected` treatment — fill + `▌` bar, animated by the travel
            // band above — while the OPEN request (the one loaded in the
            // editor) gets no fill/bar of its own at all, just its name in
            // `theme.accent`, painted below. When the two coincide (the
            // common case right after Enter) the cursor's fill simply wins
            // and the name stays normal-colored on top of it.
            let is_open = matches!(
                row,
                Row::Request { slug, .. } if self.open_slug.as_deref() == Some(slug.as_str())
            );
            let is_cursor_row = cursor_settled_at == Some(i);
            let is_hovered = ctx.hovered == Some(&Hit::SidebarRow(i));
            let text_row_f = text_row as f32;
            let intersects_band =
                band_range.is_some_and(|(y0, y1)| y0 < text_row_f + 1.0 && y1 > text_row_f);
            let is_cursor_selected = is_cursor_row && settled;
            let row_selected_visual = intersects_band || is_cursor_selected;

            let highlight = if intersects_band {
                RowHighlight::None
            } else if is_cursor_selected {
                RowHighlight::Selected
            } else if is_hovered {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };
            // The open marker only shows once it isn't already subsumed by
            // the cursor's own selection fill.
            let open_accent = is_open && !row_selected_visual;

            // The list's own inset keeps column `area.x` free for the pane
            // focus bar, so every row — selected or hover — spans the full
            // list width and the selected row's accent marker never needs
            // to dodge it. Rows the travel band is currently sitting on
            // skip their own fill entirely: the band already painted their
            // background (and, mid-flight, its fractional-edge glyphs
            // would only get stomped by a flat `ListRow` fill underneath).
            if !intersects_band {
                ListRow {
                    highlight,
                    zebra: zebra[i],
                }
                .paint(
                    buf,
                    text_row,
                    list_area.x,
                    list_area.width,
                    theme.panel,
                    hover_t,
                    theme,
                );
            }

            let row_fill = if intersects_band {
                theme.selection
            } else {
                Self::resolve_fill(theme, highlight, zebra[i], theme.panel, hover_t)
            };

            self.paint_row(
                buf,
                row,
                text_row,
                list_area,
                row_fill,
                is_open,
                open_accent,
                theme,
            );

            // Hit math is a direct `y - list_top` on the 1-line pitch now —
            // no half-pad rows to fold in.
            let row_rect = Rect {
                x: list_area.x,
                y: text_row,
                width: list_area.width,
                height: 1,
            };
            hits.register(row_rect, Hit::SidebarRow(i));

            if let Row::Folder { depth, .. } = row {
                let arrow_x = list_area.x + 1 + (*depth as u16) * 2;
                if arrow_x < list_area.x + list_area.width {
                    let arrow_rect = Rect {
                        x: arrow_x,
                        y: text_row,
                        width: 1,
                        height: 1,
                    };
                    hits.register(arrow_rect, Hit::SidebarFolderArrow(i));
                }
            }
        }
    }
}

/// Column width of the request row's method tag field (text right-padded
/// to this many columns before the display name starts).
const TAG_WIDTH: u16 = 5;

impl Sidebar {
    /// Number of rows that fit in a list area `list_height` lines tall, on
    /// the dense 1-line pitch: every line is a row, so this is just the
    /// height itself.
    fn visible_rows(list_height: u16) -> usize {
        list_height as usize
    }

    /// The request row's method tag text: a short (3–5 char) label that
    /// always fits inside [`TAG_WIDTH`] columns, unlike [`Method::as_str`]
    /// (`DELETE`, `OPTIONS` overflow it).
    fn method_tag(m: Method) -> &'static str {
        match m {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DEL",
            Method::Head => "HEAD",
            Method::Options => "OPT",
        }
    }

    /// One zebra-parity slot per row in `self.rows`: `Some(bool)` for a
    /// request row (alternating within its group), `None` for a folder row
    /// (folder rows keep no zebra fill of their own — they're the block
    /// separators). The parity counter restarts at 0 every time a
    /// top-level (depth 0) folder row is crossed, so each top-level
    /// folder's expanded subtree reads as its own zebra-striped group;
    /// nested folder rows don't reset it, since they're inside the same
    /// group as their siblings.
    fn zebra_parities(&self) -> Vec<Option<bool>> {
        let mut out = Vec::with_capacity(self.rows.len());
        let mut counter = 0usize;
        for row in &self.rows {
            match row {
                Row::Folder { depth: 0, .. } => {
                    counter = 0;
                    out.push(None);
                }
                Row::Folder { .. } => out.push(None),
                Row::Request { .. } => {
                    out.push(Some(counter % 2 == 1));
                    counter += 1;
                }
            }
        }
        out
    }

    /// Replicates [`ListRow::paint`]'s own fill computation, so a row
    /// painted through it can be asked separately what color its content
    /// (text) should sit on — needed because `paint_row` sits *outside*
    /// `ListRow`, painted as a second pass on top of it.
    fn resolve_fill(
        theme: &Theme,
        highlight: RowHighlight,
        zebra: Option<bool>,
        base: Color,
        hover_t: f32,
    ) -> Color {
        let zebra_fill = match zebra {
            Some(true) => theme.zebra_alt,
            Some(false) | None => base,
        };
        match highlight {
            RowHighlight::None => zebra_fill,
            RowHighlight::Hover => crate::theme::mix(zebra_fill, theme.control, hover_t),
            RowHighlight::Selected => theme.selection,
        }
    }

    /// The row list's scroll state, as of the last draw. `None` before the
    /// first frame (the viewport height is a render-time fact). Content and
    /// viewport are both counted in rows, which on the 1-line pitch are
    /// also lines — `offset` stays in the same unit as `scroll`, which
    /// callers (e.g. thumb-drag handling) assign directly.
    pub fn scrollbar_spec(&self) -> Option<ScrollbarSpec> {
        if self.last_list_height == 0 {
            return None;
        }
        Some(ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: self.scroll,
            content: self.rows.len(),
            viewport: Self::visible_rows(self.last_list_height as u16),
        })
    }

    /// Clips `s` to at most `max` chars (char-count width, matching the
    /// paint layer's char-count convention elsewhere).
    fn clip(s: &str, max: u16) -> &str {
        if max == 0 {
            return "";
        }
        match s.char_indices().nth(max as usize) {
            Some((idx, _)) => &s[..idx],
            None => s,
        }
    }

    /// Paints one row's text-row content (method tag/disclosure + name) at
    /// `text_row`, on top of the fill already painted there (by `ListRow`
    /// or the travel band). `row_fill` is that fill — the surface the tag
    /// text and name sit on. `open` marks the OPEN request (bolds its
    /// name); `open_accent` is `open` further narrowed to "and not already
    /// shown via the cursor's own selection fill" — when set, the name
    /// paints in `theme.accent` instead of `theme.text` since it has no
    /// fill/bar of its own to carry that signal.
    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        buf: &mut Buffer,
        row: &Row,
        text_row: u16,
        list_area: Rect,
        row_fill: Color,
        open: bool,
        open_accent: bool,
        theme: &Theme,
    ) {
        let right = list_area.x + list_area.width;
        // Column 0 is reserved for the selection's accent bar (painted by
        // `ListRow` or the travel band), whether or not this row is
        // selected, so content never shifts when selection changes.
        let text_x = list_area.x + 1;

        match row {
            Row::Folder {
                name,
                depth,
                expanded,
                ..
            } => {
                let x = text_x + (*depth as u16) * 2;
                if x >= right {
                    return;
                }
                let glyph = if *expanded { "\u{2304}" } else { "\u{203a}" };
                text(buf, x, text_row, glyph, theme.text_muted, row_fill, false);
                let name_x = x + 2;
                if name_x < right {
                    let label = format!("{name}/");
                    text(
                        buf,
                        name_x,
                        text_row,
                        Self::clip(&label, right - name_x),
                        theme.text_muted,
                        row_fill,
                        false,
                    );
                }
            }
            Row::Request {
                slug,
                name,
                depth,
                broken,
                method,
            } => {
                let x = text_x + (*depth as u16) * 2;
                if x >= right {
                    return;
                }
                let basename = name.as_str();

                let content_x = match (method, broken) {
                    (Some(m), None) => {
                        let label = format!(
                            "{:<width$}",
                            Self::method_tag(*m),
                            width = TAG_WIDTH as usize
                        );
                        text(
                            buf,
                            x,
                            text_row,
                            Self::clip(&label, right.saturating_sub(x)),
                            theme.method_color(*m),
                            row_fill,
                            true,
                        );
                        x + TAG_WIDTH + 1
                    }
                    // Broken (unparseable) files have no method to chip: an
                    // error glyph stands in its place instead.
                    _ => {
                        text(buf, x, text_row, "\u{2717}", theme.error, row_fill, false);
                        x + 2
                    }
                };
                if content_x >= right {
                    return;
                }

                let dirty = broken.is_none()
                    && self.open_slug.as_deref() == Some(slug.as_str())
                    && self.open_dirty;
                let name_x = if dirty {
                    text(
                        buf,
                        content_x,
                        text_row,
                        "\u{25cf} ",
                        theme.accent,
                        row_fill,
                        false,
                    );
                    content_x + 2
                } else {
                    content_x
                };
                if name_x >= right {
                    return;
                }
                let name_fg = if broken.is_some() {
                    theme.error
                } else if open_accent {
                    theme.accent
                } else {
                    theme.text
                };
                text(
                    buf,
                    name_x,
                    text_row,
                    Self::clip(basename, right - name_x),
                    name_fg,
                    row_fill,
                    open,
                );
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
                name: None,
                slug: s.to_string(),
                broken: None,
                method: Some(Method::Get),
            })
            .collect()
    }

    fn expanded(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rows_show_display_names_sorted_by_name_not_slug() {
        let mut s = Sidebar::default();
        let mut l = listing(&["zeta-slug", "alpha-slug", "legacy"]);
        // Display names deliberately invert the slug order.
        l[0].name = Some("Aardvark".into()); // zeta-slug
        l[1].name = Some("Zebra".into()); // alpha-slug
        // l[2] "legacy" stays nameless -> displays its slug leaf.
        s.refresh(l, &expanded(&[]));
        let names: Vec<&str> = s
            .rows
            .iter()
            .map(|r| match r {
                Row::Request { name, .. } => name.as_str(),
                Row::Folder { name, .. } => name.as_str(),
            })
            .collect();
        assert_eq!(
            names,
            ["Aardvark", "legacy", "Zebra"],
            "requests sort by display name, case-insensitive"
        );
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
                    name: "top".into(),
                    depth: 0,
                    broken: None,
                    method: Some(Method::Get),
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
                    name: "top".into(),
                    depth: 0,
                    broken: None,
                    method: Some(Method::Get),
                },
                Row::Folder {
                    path: "api".into(),
                    name: "api".into(),
                    depth: 0,
                    expanded: true
                },
                Row::Request {
                    slug: "api/ping".into(),
                    name: "ping".into(),
                    depth: 1,
                    broken: None,
                    method: Some(Method::Get),
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
        s.handle_key(key(KeyCode::Char('j'))); // first press lands on row 0 ("top")
        s.handle_key(key(KeyCode::Char('j'))); // now on the "api" folder row
        assert!(matches!(
            s.rows[s.selected.expect("a selected row")],
            Row::Folder { .. }
        ));
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
        let ctx = draw_ctx(&theme, None);
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

    /// A disabled (instantly-jumping) `Anims` shared by every test's
    /// `DrawCtx`, so tests stay deterministic without threading an owned
    /// `Anims` through each call site.
    fn test_anims() -> &'static crate::anim::Anims {
        static ANIMS: std::sync::OnceLock<crate::anim::Anims> = std::sync::OnceLock::new();
        ANIMS.get_or_init(|| crate::anim::Anims::new(false))
    }

    fn draw_ctx<'a>(theme: &'a Theme, hovered: Option<&'a Hit>) -> DrawCtx<'a> {
        DrawCtx {
            theme,
            focused: true,
            hovered,
            dragging: false,
            anims: test_anims(),
            now: std::time::Instant::now(),
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
        // 30 wide, no border/padding now -> the bar owns the pane's last
        // column outright.
        assert_eq!(thumb.x, 29);
        let track = hits.track_of(PaneId::Sidebar).expect("track rect");
        assert_eq!(track.x, thumb.x);
        // 12-row pane: label(1) + button(3) + spacer(1) = 5 rows overhead,
        // leaving 7 lines for the list -> 7 rows fit on the dense 1-line
        // pitch.
        let viewport = 7i16;
        assert!(
            thumb.height < track.height,
            "30 rows in a 7-row viewport is a short thumb"
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
        let (_content, hits) = render_hits(&mut s);
        assert_eq!(hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar)), None);
        assert_eq!(hits.track_of(PaneId::Sidebar), None);
        // Rows share the button's 1-column inset each side (the scrollbar,
        // when one is needed, lives in the right margin column).
        let row = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        assert_eq!(row.width, 28);
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

        let button_rect = hits.rect_of(&Hit::SidebarNewRequest).expect("button hit");
        let label_row: String = (button_rect.x..button_rect.x + button_rect.width)
            .map(|x| {
                terminal.backend().buffer()[(x, button_rect.y + 1)]
                    .symbol()
                    .to_string()
            })
            .collect();
        assert!(label_row.contains("+ New request"));
        // No `[`/`]` anywhere in the whole pane: scan every cell's symbol
        // directly (not a `Debug`-formatted dump, whose own `Vec` brackets
        // would give a false positive).
        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let sym = buf[(x, y)].symbol();
                assert_ne!(sym, "[", "no bracket glyph at ({x},{y})");
                assert_ne!(sym, "]", "no bracket glyph at ({x},{y})");
            }
        }
        assert_eq!(
            button_rect.y, 1,
            "button sits below the REQUESTS label at the top of the pane"
        );
        assert_eq!(button_rect.height, 3, "the paint-layer button is 3 rows");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(button_rect.x, button_rect.y + 2)].symbol(),
            "\u{2581}",
            "button's bottom row is its thin bevel edge"
        );

        // rows[0] = "top", rows[1] = folder "api" (expanded), rows[2] = "api/ping"
        // list starts at y = label(1) + button(3) + spacer(1) = 5.
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).expect("row 0 hit");
        assert_eq!(row0.y, 5, "row 0 sits right at the list top, 1-line pitch");
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).expect("row 1 hit");
        let row2 = hits.rect_of(&Hit::SidebarRow(2)).expect("row 2 hit");
        assert_eq!(row1.y - row0.y, 1, "rows sit on a 1-line pitch");
        assert_eq!(row2.y - row1.y, 1);

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
        // Two rows so the hovered one (row 1) differs from the
        // default-selected one (row 0) — selection otherwise wins the fill.
        s.refresh(listing(&["top", "next"]), &expanded(&[]));
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, Some(&Hit::SidebarRow(1)));
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        // The hit rect *is* the text row on the 1-line pitch. Sampled at
        // the row's right end: column 0 is the selection/focus lane hover
        // rows leave untouched, and the columns after it hold the row's
        // own fill.
        let text_row = row1.y;
        let cell = terminal.backend().buffer()[(row1.x + row1.width - 1, text_row)].clone();
        assert_eq!(
            cell.bg, theme.control,
            "hovered row uses the control fill, not inverted fg/bg"
        );
        assert_ne!(
            cell.fg, theme.panel,
            "text isn't painted by inverting fg/bg"
        );
    }

    /// The keyboard cursor row (`self.selected`, once its travel animation
    /// has settled — which it has here: the test's `Anims` is disabled, so
    /// `draw` degenerates straight to the static selected row with no
    /// `frac_vspan` band in play) keeps the full `▌` bar + `theme.selection`
    /// fill. The OPEN request, on a *different* row, gets none of that —
    /// only its name colored `theme.accent` — so the two stay visually
    /// distinct rather than reading as the same "selected" row twice.
    #[test]
    fn open_row_and_cursor_row_stay_visually_distinct() {
        let mut s = Sidebar::default();
        // Rows sort by slug: row 0 is "next", row 1 is "top".
        s.refresh(listing(&["top", "next"]), &expanded(&[]));
        s.open_slug = Some("next".into());
        s.selected = Some(1); // cursor browsed onto "top"
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, None);
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();

        // Row 0 ("next", open, not cursor): no bar, no selection fill —
        // just its name in `theme.accent`. The tag column is `TAG_WIDTH`
        // (5) chars wide plus a 1-column gap after the reserved bar column,
        // so the name starts at `row.x + 1 (bar lane) + 1 (text_x) + 6`.
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let bar_cell = buf[(row0.x, row0.y)].clone();
        let fill_cell = buf[(row0.x + row0.width - 2, row0.y)].clone();
        let name_cell = buf[(row0.x + 7, row0.y)].clone();
        assert_ne!(
            bar_cell.symbol(),
            "\u{258c}",
            "no accent bar on the open-but-not-cursor row"
        );
        assert_ne!(
            fill_cell.bg, theme.selection,
            "no selection fill on the open-but-not-cursor row"
        );
        assert_eq!(
            name_cell.symbol(),
            "n",
            "sanity: sampled the name cell (\"next\")"
        );
        assert_eq!(
            name_cell.fg, theme.accent,
            "open row's name is accent-colored in place of a fill/bar"
        );

        // Row 1 ("top", cursor, not open): the ordinary `▌` bar + selection
        // fill, and its name is NOT accent-colored (it's not the open row).
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        let bar_cell = buf[(row1.x, row1.y)].clone();
        let fill_cell = buf[(row1.x + row1.width - 2, row1.y)].clone();
        let name_cell = buf[(row1.x + 7, row1.y)].clone();
        assert_eq!(
            bar_cell.symbol(),
            "\u{258c}",
            "accent bar on the cursor row"
        );
        assert_eq!(bar_cell.fg, theme.accent);
        assert_eq!(fill_cell.bg, theme.selection);
        assert_eq!(
            name_cell.symbol(),
            "t",
            "sanity: sampled \"top\"'s name cell"
        );
        assert_eq!(
            name_cell.fg, theme.text,
            "cursor row's name stays normal-colored; the fill/bar already carries the signal"
        );
    }

    /// When the cursor lands on the open request itself (the common case
    /// right after Enter), the cursor's selection fill simply wins — no
    /// separate open-accent styling layered underneath it.
    #[test]
    fn cursor_on_the_open_row_shows_plain_selection_not_accent_name() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["only"]), &expanded(&[]));
        s.open_slug = Some("only".into());
        s.selected = Some(0);
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, None);
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let bar_cell = buf[(row0.x, row0.y)].clone();
        let fill_cell = buf[(row0.x + row0.width - 2, row0.y)].clone();
        let name_cell = buf[(row0.x + 7, row0.y)].clone();
        assert_eq!(bar_cell.symbol(), "\u{258c}");
        assert_eq!(fill_cell.bg, theme.selection);
        assert_eq!(
            name_cell.fg, theme.text,
            "coinciding open+cursor: the selection fill wins, name stays normal-colored"
        );
    }

    #[test]
    fn request_row_paints_a_method_tag() {
        let mut s = Sidebar::default();
        // Two rows, neither selected/hovered/open: row 0 sits on the base
        // fill (zebra parity false), row 1 on the zebra stripe (parity
        // true) — check the tag's colors against the surface it actually
        // sits on.
        s.refresh(
            vec![
                RequestListing {
                    name: None,
                    slug: "ping".into(),
                    broken: None,
                    method: Some(Method::Get),
                },
                RequestListing {
                    name: None,
                    slug: "pong".into(),
                    broken: None,
                    method: Some(Method::Get),
                },
            ],
            &expanded(&[]),
        );
        let (_, hits) = render_hits(&mut s);
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        let theme = Theme::dark();
        // Tag starts one column after the reserved accent-bar column.
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let ctx = draw_ctx(&theme, None);
        let mut hits2 = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits2))
            .unwrap();
        let tag_cell = terminal.backend().buffer()[(row1.x + 1, row1.y)].clone();
        assert_eq!(tag_cell.symbol(), "G", "GET tag's first letter");
        assert_eq!(
            tag_cell.fg,
            theme.method_color(Method::Get),
            "tag text is colored by method, not filled"
        );
        assert_eq!(
            tag_cell.bg, theme.zebra_alt,
            "tag sits directly on the row's own (zebra) fill"
        );
    }

    #[test]
    fn broken_request_row_has_no_chip() {
        let mut s = Sidebar::default();
        s.refresh(
            vec![RequestListing {
                name: None,
                slug: "bad".into(),
                broken: Some("parse error".into()),
                method: None,
            }],
            &expanded(&[]),
        );
        let (content, _) = render_hits(&mut s);
        assert!(
            content.contains('\u{2717}'),
            "broken rows keep the error glyph in place of a chip"
        );
    }

    #[test]
    fn visible_rows_is_the_full_list_height_on_the_1_line_pitch() {
        assert_eq!(Sidebar::visible_rows(7), 7);
        assert_eq!(Sidebar::visible_rows(0), 0);
        assert_eq!(Sidebar::visible_rows(1), 1);
    }

    #[test]
    fn zebra_parity_restarts_at_each_top_level_folder_and_skips_folder_rows() {
        let mut s = Sidebar::default();
        // top-level requests "a","b","c" (parity false,true,false), then a
        // top-level folder "grp" (no zebra of its own) whose two expanded
        // children restart the parity at false.
        s.refresh(
            listing(&["a", "b", "c", "grp/x", "grp/y"]),
            &expanded(&["grp"]),
        );
        let zebra = s.zebra_parities();
        // rows: [a, b, c, grp(folder), grp/x, grp/y]
        assert_eq!(
            zebra,
            vec![
                Some(false), // a
                Some(true),  // b
                Some(false), // c
                None,        // grp (folder, no zebra of its own)
                Some(false), // grp/x — restarted, not continuing c's parity
                Some(true),  // grp/y
            ]
        );
    }

    #[test]
    fn consecutive_rows_get_adjacent_1_line_hit_rects() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), &expanded(&[]));
        let (_, hits) = render_hits(&mut s);
        let r0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let r1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        let r2 = hits.rect_of(&Hit::SidebarRow(2)).unwrap();
        assert_eq!(r0.height, 1);
        assert_eq!(r1.y, r0.y + 1, "rows are exactly adjacent, no gap");
        assert_eq!(r2.y, r1.y + 1);
    }

    /// Mid-flight, the traveling band shows the accent-colored fractional
    /// glyphs `frac_vspan` paints for a partially-covered row, rather than
    /// the flat `▌` + `theme.selection` fill a settled row gets.
    #[test]
    fn mid_flight_selection_travel_paints_a_fractional_band() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), &expanded(&[]));
        s.selected = Some(1);

        let theme = Theme::dark();
        let now = std::time::Instant::now();
        // Retarget the travel key straight through `Anims` (bypassing
        // `App`, which isn't in scope for a sidebar-only unit test): a
        // 50ms-old start against a 100ms move, so `now` samples mid-flight
        // rather than at t=0 or t=1.
        let mut anims = crate::anim::Anims::new(true);
        anims.snap(
            crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar),
            0.0,
        );
        let started = now - std::time::Duration::from_millis(50);
        anims.retarget(
            crate::anim::AnimKey::ListTravel(crate::anim::ListId::Sidebar),
            1.0,
            std::time::Duration::from_millis(100),
            started,
        );
        let ctx = DrawCtx {
            theme: &theme,
            focused: true,
            hovered: None,
            dragging: false,
            anims: &anims,
            now,
        };

        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();

        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        // Somewhere between row 0 and row 1's text lines, the band's
        // fractional edge glyph shows an accent-tinted lower-block
        // character rather than a plain space — proof the band is
        // actually mid-transit rather than degenerated to a static row.
        let has_fractional_glyph = (row0.y..=row1.y).any(|y| {
            let sym = buf[(row0.x, y)].symbol();
            sym != " " && sym != "\u{258c}"
        });
        assert!(
            has_fractional_glyph,
            "expected a lower-block fractional glyph somewhere in the travel band"
        );
    }
}
