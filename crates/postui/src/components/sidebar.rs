use super::{Component, DrawCtx};
use crate::action::{Action, FoldOp, Verb};
use crate::anim::{AnimKey, ListId};
use crate::hit::{self, Hit, HitMap, ScrollbarSpec};
use crate::layout::PaneId;
use crate::paint::{
    BUTTON_HEIGHT, Button, ButtonKind, ControlState, ListRow, RowHighlight, fill, text,
};
use crate::theme::Theme;
use postui_core::model::Method;
use postui_core::storage::RequestListing;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

/// Identifies a row across a rebuild (a folder path or a request slug).
/// Undo's `View` stores the cursor as one (spec §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Folder(String),
    Request(String),
}

/// What a fold key does to the sidebar's folders (plan M5b Deviation 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldChange {
    /// No folder holds the cursor's row: Vim's E490, "No fold found".
    NoFold,
    /// Nothing to open or close.
    Same,
    /// The open folders after the key.
    Expanded(BTreeSet<String>),
}

#[derive(Default)]
pub struct Sidebar {
    pub rows: Vec<Row>,
    /// Replaces the "No requests yet." empty state while set: the app's
    /// explanation of why nothing is loaded (a project that refused to
    /// open). Paragraphs are separated by blank lines.
    pub notice: Option<String>,
    /// Index of the cursor/selected row. `None` only when there are no
    /// rows at all; whenever rows exist the cursor is on one of them (G5).
    /// A refresh preserves the selection by row identity, else hands it to
    /// the neighbouring row (see `rebuild`).
    pub selected: Option<usize>,
    /// Index of the first row drawn; `draw` keeps `selected` inside the
    /// visible window whenever `ensure_visible` is set, by adjusting this.
    pub scroll: usize,
    /// The full flat listing behind the current tree, kept so future
    /// rebuilds don't need a caller-supplied copy.
    listing: Vec<RequestListing>,
    /// Ancestor folder paths queued open to make a target row visible:
    /// by `expand_to` (a landing's cursor row, `App::land` step 4) and by
    /// `select_slug`, which calls it. The caller (`App::refresh_sidebar`)
    /// merges these into `project.expanded` and clears this set on the
    /// next refresh.
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
    /// Slugs whose sends are still waiting on a result, likewise kept in
    /// sync by `App::update` — their rows show a pending glyph so
    /// background sends stay visible while the user works elsewhere.
    pub in_flight: BTreeSet<String>,
    /// The row the selection band is crossfading away from: the previously
    /// open request's row, recorded by `App::retarget_sidebar_travel` when
    /// the open request changes. Only read while `ListTravel` is still
    /// easing — once the anim settles the band paints solely on the open
    /// row and this goes stale harmlessly.
    pub band_fade_from: Option<usize>,
    /// A live row drag (spec §Sidebar): while `Some`, `rebuild` shows
    /// `working` for that one level instead of the order list.
    pub drag: Option<RowDrag>,
    /// Screen row of the first list line as of the last draw — with
    /// `scroll`, maps a pointer row back to a row index (`row_at_y`).
    last_list_top: u16,
    /// The row the cursor is really on while a closed folder hides it, and
    /// the folder row it shows on (plan M5b Deviation 10): Vim's cursor
    /// keeps its line inside a closed fold, so `zc` then `zo` comes back.
    /// Any cursor move forgets it, and it counts only while the folder row
    /// is still the selected one.
    fold_row: Option<(RowKey, RowKey)>,
}

/// A live drag of one sidebar request row: the working order the pointer
/// has arranged so far, for the one level the dragged row belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDrag {
    pub slug: String,          // full slug of the dragged request
    pub space: String,         // the space the drag started in
    pub level: String,         // level within the space ("" root, "auth", …)
    pub original: Vec<String>, // sibling order at drag start (relative slugs)
    pub working: Vec<String>,  // current on-screen order (relative slugs)
}

/// The parts of a row rebuild that are the same at every folder level:
/// which folders are open, the space being drawn, its saved order list,
/// and the live-drag overlay (level, working order) that replaces the
/// saved order for the one level a drag is rearranging.
#[derive(Clone, Copy)]
struct LevelCtx<'a> {
    expanded: &'a BTreeSet<String>,
    space: &'a str,
    order: &'a [String],
    overlay: Option<(&'a str, &'a [String])>,
}

impl Sidebar {
    /// Rebuilds `rows` as a tree from a fresh listing: at each level, this
    /// level's requests come first — the ones in `order` in list order,
    /// then the rest by display name — then its subfolders (sorted by
    /// name); a folder's children are emitted only when `expanded`
    /// contains its path. Preserves the current selection by row identity
    /// (folder path or request slug) across the rebuild; a selection whose
    /// row vanished lands on the neighbouring row instead, and while any
    /// rows exist the cursor is never left empty (G5).
    ///
    /// Only `space`'s entries become rows: everything outside it is dropped
    /// (but kept in `listing` for `space_counts`), and the tree is rooted at
    /// `space/` so the space itself is never a row and its own top level is
    /// depth 0.
    pub fn refresh(
        &mut self,
        listing: Vec<RequestListing>,
        space: &str,
        expanded: &BTreeSet<String>,
        order: &[String],
    ) {
        self.listing = listing;
        self.rebuild(space, expanded, order);
    }

    /// Rebuilds the rows from the listing already held — the cheap path
    /// for a change that touched no file (a live drag, a folder toggle).
    /// Same selection-preserving rules as `refresh`.
    pub fn rebuild(&mut self, space: &str, expanded: &BTreeSet<String>, order: &[String]) {
        let prev = self.selected_key();
        let prev_index = self.selected;
        let prefix = format!("{space}/");
        let mut sorted: Vec<RequestListing> = self
            .listing
            .iter()
            .filter(|l| l.slug.starts_with(&prefix))
            .cloned()
            .collect();
        sorted.sort_by(|a, b| a.slug.cmp(&b.slug));

        // The overlay is a set of slugs relative to the space the drag
        // started in; if the space changed under a live drag (a space
        // switch with the button still held) it means nothing here, so the
        // rows fall back to the new space's saved order.
        let overlay = self
            .drag
            .as_ref()
            .filter(|d| d.space == space)
            .map(|d| (d.level.as_str(), d.working.as_slice()));
        let mut rows = Vec::new();
        let ctx = LevelCtx {
            expanded,
            space,
            order,
            overlay,
        };
        Self::build_rows(&sorted, &prefix, 0, &ctx, &mut rows);
        self.rows = rows;

        self.selected =
            prev.and_then(|id| self.rows.iter().position(|r| Self::row_matches(r, &id)));
        // G5: while rows exist the cursor is somewhere. A row that vanished
        // (an outside delete, a switch to another space's rows) hands the
        // cursor to the request now at its index, else the nearest one; a
        // list with no request rows puts it on the row at that index. Every
        // in-app op lands explicitly (`App::land`); this only serves
        // refreshes nothing aimed.
        if self.selected.is_none() && !self.rows.is_empty() {
            let at = prev_index.unwrap_or(0).min(self.rows.len() - 1);
            self.select_nearest_request(at);
            if self.selected.is_none() {
                self.selected = Some(at);
            }
        }
        self.ensure_visible = true;
    }

    /// Requests per space over the full listing (every space, not just the
    /// visible one) — the Manage screen's count line and the delete
    /// confirm read this.
    pub fn space_counts(&self) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for l in &self.listing {
            if let Some(space) = postui_core::storage::space_of(&l.slug) {
                *out.entry(space.to_string()).or_insert(0) += 1;
            }
        }
        out
    }

    /// Every space's requests by display name, in the sidebar's own order
    /// (display name, then slug as the tiebreak) — the Manage screen's
    /// Spaces tab lists them under the selected space.
    pub fn space_requests(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut out: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for l in &self.listing {
            if let Some(space) = postui_core::storage::space_of(&l.slug) {
                let leaf = l.slug.rsplit('/').next().unwrap_or(&l.slug);
                let name = l.name.clone().unwrap_or_else(|| leaf.to_string());
                out.entry(space.to_string())
                    .or_default()
                    .push((name, l.slug.clone()));
            }
        }
        out.into_iter()
            .map(|(space, mut names)| {
                names.sort_by_key(|(n, slug)| (n.to_lowercase(), slug.clone()));
                (space, names.into_iter().map(|(n, _)| n).collect())
            })
            .collect()
    }

    /// The first request of `space` in display order with `expanded`
    /// folders open: the first request a user would see (§4.2 "first
    /// visible"). Same order `build_rows` paints.
    pub fn first_visible_request_in_space(
        &self,
        space: &str,
        order: &[String],
        expanded: &BTreeSet<String>,
    ) -> Option<String> {
        let prefix = format!("{space}/");
        let mut sorted: Vec<RequestListing> = self
            .listing
            .iter()
            .filter(|l| l.slug.starts_with(&prefix))
            .cloned()
            .collect();
        sorted.sort_by(|a, b| a.slug.cmp(&b.slug));
        let ctx = LevelCtx {
            expanded,
            space,
            order,
            overlay: None,
        };
        let mut rows = Vec::new();
        Self::build_rows(&sorted, &prefix, 0, &ctx, &mut rows);
        rows.into_iter().find_map(|r| match r {
            Row::Request { slug, .. } => Some(slug),
            Row::Folder { .. } => None,
        })
    }

    /// Every folder of `space`, open or not: the paths `zR` opens.
    fn folders_in(&self, space: &str) -> BTreeSet<String> {
        let prefix = format!("{space}/");
        let mut all = BTreeSet::new();
        for l in self.listing.iter().filter(|l| l.slug.starts_with(&prefix)) {
            let mut segs: Vec<&str> = l.slug[prefix.len()..].split('/').collect();
            segs.pop();
            let mut path = space.to_string();
            for seg in segs {
                path = format!("{path}/{seg}");
                all.insert(path.clone());
            }
        }
        all
    }

    /// The first request of `space` as if every folder were expanded: the
    /// switch fallback when no visible row is a request.
    pub fn first_request_in_space(&self, space: &str, order: &[String]) -> Option<String> {
        self.first_visible_request_in_space(space, order, &self.folders_in(space))
    }

    /// Builds the rows for one folder level (`prefix`, possibly empty for
    /// the root): direct requests first — the ones in `order` in list
    /// order, then the rest by display name — then subfolders in name
    /// order, recursing into each expanded one. `entries` must already be
    /// sorted by slug (the folder grouping walks slug paths) and every
    /// entry must start with `prefix`.
    fn build_rows(
        entries: &[RequestListing],
        prefix: &str,
        depth: usize,
        ctx: &LevelCtx<'_>,
        rows: &mut Vec<Row>,
    ) {
        let LevelCtx {
            expanded,
            space,
            order,
            overlay,
        } = *ctx;
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
        let this_level = prefix
            .strip_prefix(&format!("{space}/"))
            .unwrap_or("")
            .trim_end_matches('/');
        let order_here: &[String] = match overlay {
            Some((level, working)) if level == this_level => working,
            _ => order,
        };
        let requests = postui_core::order::order_level(&requests, order_here, space);
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
                Self::build_rows(&children, &child_prefix, depth + 1, ctx, rows);
            }
        }
    }

    /// The currently selected row's identity (a folder path or a request
    /// slug), stable across a rebuild even though row indices shift.
    pub fn selected_key(&self) -> Option<RowKey> {
        match self.rows.get(self.selected?)? {
            Row::Folder { path, .. } => Some(RowKey::Folder(path.clone())),
            Row::Request { slug, .. } => Some(RowKey::Request(slug.clone())),
        }
    }

    /// Selects the row `key` names, if it is visible. `true` when it did.
    pub fn select_key(&mut self, key: &RowKey) -> bool {
        match self.rows.iter().position(|r| Self::row_matches(r, key)) {
            Some(i) => {
                self.selected = Some(i);
                self.ensure_visible = true;
                self.fold_row = None;
                true
            }
            None => false,
        }
    }

    /// The first request row, else the first row; nothing when empty.
    pub fn select_first(&mut self) {
        self.fold_row = None;
        if self.rows.is_empty() {
            return;
        }
        self.select_nearest_request(0);
        if self.selected.is_none() {
            self.selected = Some(0);
            self.ensure_visible = true;
        }
    }

    /// Queues `slug`'s ancestor folders open for the next refresh, without
    /// moving the cursor (the landing places it once the rows exist). Only
    /// folder rows' paths (`space/f`, `space/f/g`) are queued, never the
    /// space itself: the tree is rooted at `space/`, so the space is no
    /// row. A slug outside any space has no folder to open.
    pub fn expand_to(&mut self, slug: &str) {
        let Some(space) = postui_core::storage::space_of(slug) else {
            return;
        };
        let mut segs: Vec<&str> = slug[space.len() + 1..].split('/').collect();
        segs.pop();
        let mut path = space.to_string();
        for seg in segs {
            path = format!("{path}/{seg}");
            self.pending_expand.insert(path.clone());
        }
    }

    /// Whether two rows name the same thing (the same folder path or the
    /// same request slug), whatever their index or paint state.
    pub fn same_row(a: &Row, b: &Row) -> bool {
        match (a, b) {
            (Row::Folder { path: p, .. }, Row::Folder { path: q, .. }) => p == q,
            (Row::Request { slug: s, .. }, Row::Request { slug: t, .. }) => s == t,
            _ => false,
        }
    }

    fn row_matches(row: &Row, id: &RowKey) -> bool {
        match (row, id) {
            (Row::Folder { path, .. }, RowKey::Folder(p)) => path == p,
            (Row::Request { slug, .. }, RowKey::Request(s)) => slug == s,
            _ => false,
        }
    }

    /// Selects the request row for `slug`, if one is currently visible, and
    /// records `slug`'s ancestor folder paths in `pending_expand` so the
    /// caller can open them before the next `refresh` (which is what
    /// actually makes the row visible when it wasn't already).
    pub fn select_slug(&mut self, slug: &str) {
        self.fold_row = None;
        self.expand_to(slug);

        if let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug: s, .. } if s == slug))
        {
            self.selected = Some(i);
            self.ensure_visible = true;
        }
    }

    /// Puts the selection on the request row nearest to index `from`
    /// (that row itself, then the closest below, then the closest above),
    /// for after the row that was there disappeared. Leaves the selection
    /// alone when no request row remains.
    pub fn select_nearest_request(&mut self, from: usize) {
        self.fold_row = None;
        let is_request = |r: &Row| matches!(r, Row::Request { .. });
        let below = self
            .rows
            .iter()
            .skip(from)
            .position(is_request)
            .map(|i| i + from);
        let above = self.rows[..from.min(self.rows.len())]
            .iter()
            .rposition(is_request);
        if let Some(i) = below.or(above) {
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

    /// The slugs of up to `n` request rows from row `start` down (piece 4
    /// spec §5.2: a counted `dd`). Folder rows are skipped, a collapsed
    /// folder's requests are not rows at all, and a folder at `start`
    /// gives nothing: `dd` on a folder does nothing.
    pub fn request_slugs_from(&self, start: usize, n: usize) -> Vec<String> {
        if !matches!(self.rows.get(start), Some(Row::Request { .. })) {
            return Vec::new();
        }
        self.rows[start..]
            .iter()
            .filter_map(|r| match r {
                Row::Request { slug, .. } => Some(slug.clone()),
                Row::Folder { .. } => None,
            })
            .take(n)
            .collect()
    }

    /// The request slug a `p`/`P` anchors its copy on (piece 4 spec
    /// §5.2). A request row is its own anchor. A folder row anchors on
    /// the folder's own direct requests: the last visible one for a put
    /// below and the first for a put above, which lands the copy in that
    /// folder either way, never in a subfolder. A folder with no visible
    /// direct request, and no selection, give `None`; use
    /// `selected_is_folder` to tell the two apart.
    pub fn put_anchor(&self, above: bool) -> Option<String> {
        match self.selected_row()? {
            Row::Request { slug, .. } => Some(slug.clone()),
            Row::Folder { path, .. } => {
                let mut direct = self.rows.iter().filter_map(|r| match r {
                    Row::Request { slug, .. }
                        if slug.rsplit_once('/').map(|(f, _)| f) == Some(path.as_str()) =>
                    {
                        Some(slug.clone())
                    }
                    _ => None,
                });
                if above { direct.next() } else { direct.next_back() }
            }
        }
    }

    /// What a list verb means on the sidebar (piece 4 spec §5.2): the
    /// cursor's request, or the cursor's slot for add and put. A folder
    /// row takes add and put only.
    pub fn verb_action(&self, verb: Verb) -> Option<Action> {
        let on_request = matches!(self.selected_row(), Some(Row::Request { .. }));
        match verb {
            Verb::Add => Some(Action::PromptNewRequest),
            Verb::Rename => on_request.then_some(Action::PromptRenameRequest),
            Verb::Move => on_request.then_some(Action::PromptMoveSelectedRequestToSpace),
            Verb::Delete { count: 0 | 1 } => on_request.then_some(Action::DeleteSelectedRequest),
            Verb::Delete { count } => {
                let mut slugs = self.request_slugs_from(self.selected?, count);
                match slugs.len() {
                    0 => None,
                    1 => slugs.pop().map(Action::DeleteRequest),
                    _ => Some(Action::DeleteRequests(slugs)),
                }
            }
            Verb::Yank => self.selected_slug().map(Action::YankRequest),
            Verb::Put { above, count } => Some(Action::PutRequest { anchor: self.put_anchor(above), above, count }),
            _ => None,
        }
    }

    /// Whether the cursor is on a folder row.
    pub fn selected_is_folder(&self) -> bool {
        matches!(self.selected_row(), Some(Row::Folder { .. }))
    }

    /// Whether the cursor is on a collapsed folder row: its requests are
    /// hidden, so a put cannot anchor among them.
    pub fn selected_is_collapsed_folder(&self) -> bool {
        matches!(self.selected_row(), Some(Row::Folder { expanded: false, .. }))
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected?)
    }

    /// The row index of the OPEN request (the one loaded in the editor), or
    /// `None` when nothing is open or its row isn't visible (an ancestor
    /// folder is collapsed). This is the row the selection band anchors to:
    /// the fill + `▌` bar always mean "this is what the right panes show",
    /// independent of pane focus and of the keyboard cursor.
    pub fn open_row(&self) -> Option<usize> {
        let open = self.open_slug.as_deref()?;
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug, .. } if slug == open))
    }

    /// Reports the currently selected folder's path and the state it should
    /// flip to (`!expanded`), without changing anything itself: the caller
    /// owns the expanded set and is expected to update it, refresh, and
    /// reselect the folder (which happens automatically, by identity, on
    /// the next `refresh`).
    pub fn toggle_selected_folder(&mut self) -> Option<(String, bool)> {
        self.fold_row = None;
        match self.selected_row()? {
            Row::Folder { path, expanded, .. } => Some((path.clone(), !expanded)),
            Row::Request { .. } => None,
        }
    }

    /// Selects row `i` (a click): the cursor moved, so a row a closed folder
    /// hid is forgotten.
    pub fn select_row(&mut self, i: usize) {
        self.selected = Some(i);
        self.fold_row = None;
    }

    /// The folders that hold `key`, outermost first. A folder row is the
    /// first line of its own fold, as a Vim fold's first line is, so it sits
    /// in itself too; a request sits in its folders. The space is no folder.
    fn folds_of(key: &RowKey) -> Vec<String> {
        let path = match key {
            RowKey::Folder(path) => path.as_str(),
            RowKey::Request(slug) => slug.rsplit_once('/').map_or("", |(dir, _)| dir),
        };
        let segs: Vec<&str> = path.split('/').collect();
        (2..=segs.len()).map(|n| segs[..n].join("/")).collect()
    }

    /// The row the cursor is really on: the one a closed folder hides, while
    /// the folder row that shows it is still selected (Deviation 10), else
    /// the selected row.
    pub fn fold_cursor(&self) -> Option<RowKey> {
        let shown = self.selected_key()?;
        match &self.fold_row {
            Some((real, at)) if *at == shown => Some(real.clone()),
            _ => Some(shown),
        }
    }

    /// Forgets a row a closed folder hid: only vim's fold keys remember
    /// one, so arrows' keys on the folder row act on the folder itself.
    pub fn forget_fold_cursor(&mut self) {
        self.fold_row = None;
    }

    /// Whether the selected folder row is showing a row it hides
    /// (`fold_cursor` is not the selected row).
    fn hides_cursor_row(&self) -> bool {
        self.fold_cursor() != self.selected_key()
    }

    /// Vim's `zo` `zc` `za` with a count, and `zR` `zM`, on the folders of
    /// `space` (plan M5b Deviations 9 and 11; probes F1–F38), from the open
    /// set `expanded`. `zo` `zc` `za` pick by `FoldOp::pick`, the rule
    /// `JsonTree::fold_at` uses, over the folders holding the cursor's row;
    /// `zR` and `zM`: every folder of `space`, the cursor or none. `real` is
    /// the caller's `fold_cursor`, which it puts back after the change.
    pub fn fold_change(
        &self,
        op: FoldOp,
        count: usize,
        real: Option<&RowKey>,
        expanded: &BTreeSet<String>,
        space: &str,
    ) -> FoldChange {
        let mut set = expanded.clone();
        match op {
            FoldOp::OpenAll => set.extend(self.folders_in(space)),
            FoldOp::CloseAll => {
                let prefix = format!("{space}/");
                set.retain(|f| !f.starts_with(&prefix));
            }
            FoldOp::Open | FoldOp::Close | FoldOp::Toggle => {
                let Some(real) = real else { return FoldChange::Same };
                let ids = Self::folds_of(real);
                if ids.is_empty() {
                    return FoldChange::NoFold;
                }
                let closed: Vec<bool> = ids.iter().map(|f| !expanded.contains(f)).collect();
                if let Some((open, pick)) = op.pick(&closed, count) {
                    for i in pick {
                        if open {
                            set.insert(ids[i].clone());
                        } else {
                            set.remove(&ids[i]);
                        }
                    }
                }
            }
        }
        if set == *expanded { FoldChange::Same } else { FoldChange::Expanded(set) }
    }

    /// Puts the cursor back on `real` after a fold key, the rows rebuilt: on
    /// its own row when it shows, else on the row of the outermost closed
    /// folder that hides it, remembering `real` (probes F2, F11, F33).
    pub fn show_fold_cursor(&mut self, real: RowKey) {
        if self.select_key(&real) {
            return;
        }
        let holder = Self::folds_of(&real).into_iter().find(|f| {
            self.rows.iter().any(|r| matches!(r, Row::Folder { path, expanded: false, .. } if path == f))
        });
        if let Some(path) = holder {
            let at = RowKey::Folder(path);
            self.select_key(&at);
            self.fold_row = Some((real, at));
        } else {
            // Neither shown nor under a closed folder (deleted from outside):
            // nothing to remember.
            self.fold_row = None;
        }
    }

    /// Moves `selected` by `delta` rows over the full tree (folders
    /// included), clamped to the first/last row. With nothing selected the
    /// first press lands on the top row rather than being a dead key.
    fn move_selection(&mut self, delta: i32) {
        self.fold_row = None;
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

    /// Height of the row list as of the last draw, as a page size for
    /// ctrl+d/u/f/b and PageUp/PageDown (never zero, so a page always moves).
    fn page(&self) -> i32 {
        self.last_list_height.max(1) as i32
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
        self.fold_row = None;
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

    /// The first and last row index of the level `i` belongs to: the
    /// contiguous run of request rows at `i`'s depth. Requests of one
    /// level are always contiguous (they precede that level's folders,
    /// and a folder row separates them from the next level), so this run
    /// is exactly the drag/move group. `None` for a folder row.
    pub fn group_bounds(&self, i: usize) -> Option<(usize, usize)> {
        let depth = match self.rows.get(i)? {
            Row::Request { depth, .. } => *depth,
            Row::Folder { .. } => return None,
        };
        let same = |r: &Row| matches!(r, Row::Request { depth: d, .. } if *d == depth);
        let mut first = i;
        while first > 0 && same(&self.rows[first - 1]) {
            first -= 1;
        }
        let mut last = i;
        while last + 1 < self.rows.len() && same(&self.rows[last + 1]) {
            last += 1;
        }
        Some((first, last))
    }

    /// The row index a screen row `y` maps to, given the last draw's scroll
    /// offset and list top — clamped to the row list.
    pub fn row_at_y(&self, y: u16) -> usize {
        let rel = y.saturating_sub(self.last_list_top) as usize;
        (self.scroll + rel).min(self.rows.len().saturating_sub(1))
    }

    /// Height of the row list as of the last draw.
    pub fn last_list_height(&self) -> usize {
        self.last_list_height
    }

    /// While dragging, a pointer resting on the list's first or last
    /// screen row asks for a one-row scroll in that direction (the tick
    /// handler applies it), so a long list can be dragged through.
    pub fn drag_edge(&self, y: u16) -> Option<i32> {
        if self.drag.is_none() || self.last_list_height == 0 {
            return None;
        }
        let bottom = self.last_list_top + self.last_list_height as u16 - 1;
        if y <= self.last_list_top && self.scroll > 0 {
            Some(-1)
        } else if y >= bottom && self.scroll + self.last_list_height < self.rows.len() {
            Some(1)
        } else {
            None
        }
    }

    /// The level row `i` belongs to and that level's requests as the rows
    /// show them right now — relative slugs, in display order. This is
    /// what a drag rearranges and what a keyboard move shifts. `None` for
    /// a folder row.
    pub fn level_group(&self, i: usize, space: &str) -> Option<(String, Vec<String>)> {
        let (first, last) = self.group_bounds(i)?;
        let rel = |slug: &str| {
            postui_core::order::relative(slug, space)
                .unwrap_or(slug)
                .to_string()
        };
        let group: Vec<String> = self.rows[first..=last]
            .iter()
            .filter_map(|r| match r {
                Row::Request { slug, .. } => Some(rel(slug)),
                _ => None,
            })
            .collect();
        let Row::Request { slug, .. } = self.rows.get(i)? else {
            return None;
        };
        let level = postui_core::order::level_of(&rel(slug)).to_string();
        Some((level, group))
    }

    /// The request row for `slug`, if it is visible.
    pub fn row_of(&self, slug: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug: s, .. } if s == slug))
    }

    /// The listing the rows were last built from: every space, not just
    /// the visible one.
    pub fn listing(&self) -> &[RequestListing] {
        &self.listing
    }

    /// Starts a drag of row `i`: records its level's sibling order as both
    /// `original` and the starting `working` order. Refuses a folder row.
    pub fn begin_drag(&mut self, i: usize, space: &str) -> bool {
        let Some((level, group)) = self.level_group(i, space) else {
            return false;
        };
        let Some(Row::Request { slug, .. }) = self.rows.get(i) else {
            return false;
        };
        self.drag = Some(RowDrag {
            slug: slug.clone(),
            space: space.to_string(),
            level,
            original: group.clone(),
            working: group,
        });
        true
    }

    /// Moves the dragged request so it sits at the group slot under row
    /// `i` (clamped to the group). Returns whether the working order
    /// changed; the caller then `rebuild`s. The rows mirror `working`
    /// (the overlay is what built them), so the target row's offset from
    /// the group's first row is its index in `working`; the dragged
    /// request itself is found in `working` by slug, so a listing that
    /// changed under the drag can never make it move the wrong sibling.
    pub fn drag_to_row(&mut self, i: usize) -> bool {
        let Some((slug, space)) = self
            .drag
            .as_ref()
            .map(|d| (d.slug.clone(), d.space.clone()))
        else {
            return false;
        };
        let Some(cur_row) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Request { slug: s, .. } if *s == slug))
        else {
            return false;
        };
        let Some((first, last)) = self.group_bounds(cur_row) else {
            return false;
        };
        let rel = postui_core::order::relative(&slug, &space).unwrap_or(&slug);
        let drag = self.drag.as_mut().expect("checked above");
        let Some(cur) = drag.working.iter().position(|s| s == rel) else {
            return false;
        };
        let target = i.clamp(first, last) - first;
        if cur == target || target >= drag.working.len() {
            return false;
        }
        let moved = drag.working.remove(cur);
        drag.working.insert(target, moved);
        true
    }

    /// Puts the working order back to the order the drag started from,
    /// without ending the drag: the preview of a cancel, shown while the
    /// pointer is outside the pane (where a release would cancel).
    /// Returns whether anything changed; the caller then `rebuild`s.
    pub fn drag_reset(&mut self) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        if drag.working == drag.original {
            return false;
        }
        drag.working = drag.original.clone();
        true
    }
}

impl Component for Sidebar {
    fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        // Each navigation arm takes exactly the chord it names: a bare named
        // key, alt+↑/↓ (move the row), ctrl+Home/End (first/last row) or
        // ctrl+d/u/f/b. Any other chord does nothing rather than act as the
        // bare key (piece 4 spec §5.1).
        let plain = ev.modifiers.is_empty();
        let ctrl = ev.modifiers == KeyModifiers::CONTROL;
        if ev.modifiers == KeyModifiers::ALT
            && let Some(delta) = match ev.code {
                KeyCode::Up => Some(-1),
                KeyCode::Down => Some(1),
                _ => None,
            }
        {
            return self
                .selected_slug()
                .map(|slug| Action::MoveRequest { slug, delta });
        }
        match ev.code {
            KeyCode::Down if plain => {
                self.move_selection(1);
                Some(Action::Render)
            }
            KeyCode::Up if plain => {
                self.move_selection(-1);
                Some(Action::Render)
            }
            KeyCode::Enter if plain => match self.selected_row()? {
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
            // ctrl+d/u are the half page; ctrl+f/b and PageDown/PageUp the
            // full one — the same two distances on every list surface.
            KeyCode::Char('d') if ev.modifiers == KeyModifiers::CONTROL => {
                self.move_selection((self.page() / 2).max(1));
                Some(Action::Render)
            }
            KeyCode::Char('u') if ev.modifiers == KeyModifiers::CONTROL => {
                self.move_selection(-(self.page() / 2).max(1));
                Some(Action::Render)
            }
            KeyCode::Char('f') if ev.modifiers == KeyModifiers::CONTROL => {
                self.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
            }
            KeyCode::Char('b') if ev.modifiers == KeyModifiers::CONTROL => {
                self.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
            }
            // ctrl+Home/End are a second spelling of Home/End, and what
            // macOS's cmd+↑/↓ fold to (`keys::normalize_super_keys`).
            KeyCode::Home if plain || ctrl => {
                self.move_selection(i32::MIN / 2);
                Some(Action::Render)
            }
            KeyCode::End if plain || ctrl => {
                self.move_selection(i32::MAX / 2);
                Some(Action::Render)
            }
            KeyCode::PageDown if plain => {
                self.move_selection(self.page());
                Some(Action::Render)
            }
            KeyCode::PageUp if plain => {
                self.move_selection(-self.page());
                Some(Action::Render)
            }
            // The folder row shows a row it hides (a vim `zc`): the cursor is
            // really there, at column 0. Vim's `l` moves, and `'foldopen'`
            // opens every fold around it; its `h` cannot move, so nothing
            // opens and the cursor keeps its line (sweep backlog item 3).
            KeyCode::Right if plain && self.hides_cursor_row() => {
                Some(Action::SidebarFold { op: crate::action::FoldOp::Open, count: usize::MAX })
            }
            KeyCode::Left if plain && self.hides_cursor_row() => Some(Action::Render),
            KeyCode::Right if plain => matches!(
                self.selected_row()?,
                Row::Folder {
                    expanded: false,
                    ..
                }
            )
            .then_some(Action::ToggleSelectedFolder),
            KeyCode::Left if plain => match self.selected_row()? {
                Row::Folder { expanded: true, .. } => Some(Action::ToggleSelectedFolder),
                _ => {
                    self.jump_to_parent();
                    Some(Action::Render)
                }
            },
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

        // The button starts on the pane's first row: it used to sit below
        // a blank padding row, air a bevelled button needed to read as
        // raised, and its top cap is that air now -- three quarters of
        // the row above the label is already pane surface.
        //
        // The blank row *below* it stays, matching the one under the
        // address bar, so the two controls that share the app's top band
        // sit in the same rhythm.
        let button_top = area.y;
        let button_height = BUTTON_HEIGHT.min(area.y + area.height - button_top);
        // A notice means nothing is loaded (the open was refused): there
        // is no project for a new request to land in, so the button is
        // not offered.
        if button_height == BUTTON_HEIGHT && self.notice.is_none() {
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
            .paint(buf, button_area, theme);
            hits.register(button_area, Hit::SidebarNewRequest);
        }

        // One blank spacer row below the button; the list starts after it.
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
        self.last_list_top = list_area.y;

        if self.rows.is_empty() {
            let empty = match &self.notice {
                Some(notice) => {
                    let mut lines = vec![Line::raw("")];
                    lines.extend(notice.lines().map(|l| Line::raw(l.to_string())));
                    Paragraph::new(lines)
                        .wrap(ratatui::widgets::Wrap { trim: false })
                        .style(Style::default().fg(theme.text).bg(theme.panel))
                }
                None => Paragraph::new(vec![Line::raw(""), Line::raw("No requests yet.")])
                    .style(Style::default().fg(theme.text_muted).bg(theme.panel))
                    .centered(),
            };
            frame.render_widget(empty, list_area);
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

        let hover_t = ctx.hover_t();

        // The open request's band crossfade: while `ListTravel` is still
        // easing between the previously open row (`band_fade_from`) and
        // the newly open one, BOTH rows paint the band at partial strength
        // — fading out on the old row, in on the new — rather than sliding
        // a fractional-height band between them (whose full-cell edge
        // fills made the half-width `▌` bar read as doubling in width
        // mid-flight). Once the anim arrives — or was never wired up at
        // all, e.g. every sidebar-only unit test — the loop below paints
        // the open row exactly like any other `ListRow::Selected`. The
        // band anchors to `open_row()` (NOT the keyboard cursor) and
        // ignores pane focus: the fill + bar always mean "this request is
        // the one in the right panes".
        let travel_key = AnimKey::ListTravel(ListId::Sidebar);
        let band_settled_at = self.open_row();
        let anim_val = band_settled_at.map(|c| ctx.anims.value_or(travel_key, ctx.now, c as f32));
        let settled = match (band_settled_at, anim_val) {
            (Some(c), Some(v)) => (v - c as f32).abs() < 0.01,
            _ => true,
        };
        // Eased 0→1 progress of the crossfade; `None` once settled, or
        // with no recorded origin row to fade out from (then the band just
        // paints fully on the open row).
        let fade_t = match (band_settled_at, anim_val, self.band_fade_from) {
            (Some(c), Some(v), Some(f)) if !settled && f != c => {
                Some(((v - f as f32) / (c as f32 - f as f32)).clamp(0.0, 1.0))
            }
            _ => None,
        };

        let buf = frame.buffer_mut();

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
            // distinct: the OPEN request (the one loaded in the editor)
            // gets the full `Selected` treatment — fill + `▌` bar, animated
            // by the travel band above, shown whether or not the pane is
            // focused — while the keyboard cursor (`self.selected`), when
            // it rests on a *different* row (arrow-key browsing, or a
            // right-click arming a context menu), gets a steady
            // `control_hover`-fill marker: visibly targeted, clearly not the
            // band. When the two coincide (the common case right after an
            // open), the cursor still needs a cue of its own (R6: the
            // cursor always sits somewhere): the band's fill lifts half a
            // step toward the cursor fill, but only while the pane is
            // focused — unfocused, the open row reads exactly as before.
            let is_open = matches!(
                row,
                Row::Request { slug, .. } if self.open_slug.as_deref() == Some(slug.as_str())
            );
            let is_hovered = ctx.hovered == Some(&Hit::SidebarRow(i));
            // Full band once the crossfade is done (or was never armed);
            // partial strength on the two rows a crossfade is between.
            let is_band_row = band_settled_at == Some(i) && fade_t.is_none();
            let band_alpha = fade_t.and_then(|t| {
                if self.band_fade_from == Some(i) {
                    Some(1.0 - t)
                } else if band_settled_at == Some(i) {
                    Some(t)
                } else {
                    None
                }
            });
            let is_cursor =
                ctx.focused && self.selected == Some(i) && !is_band_row && band_alpha.is_none();
            // §4.7: the cursor always sits somewhere (R6), so when it rests
            // on the open row it needs a cue of its own: the band's fill
            // lifts half a step toward the cursor fill, only while the
            // pane is focused.
            let lifted = ctx.focused && self.selected == Some(i) && is_band_row;
            let is_dragged = matches!(
                (row, self.drag.as_ref()),
                (Row::Request { slug, .. }, Some(d)) if *slug == d.slug
            );

            // The dragged row keeps the selected fill while it travels, so
            // it reads the same as the settled selection band.
            let highlight = if is_dragged || is_band_row {
                RowHighlight::Selected
            } else if is_cursor {
                RowHighlight::Cursor
            } else if is_hovered && band_alpha.is_none() {
                RowHighlight::Hover
            } else {
                RowHighlight::None
            };

            // The list's own inset keeps column `area.x` free for the pane
            // focus bar, so every row — selected or hover — spans the full
            // list width and the selected row's accent marker never needs
            // to dodge it. A crossfading row paints its own blend instead:
            // the base fill mixed toward `theme.selection` at `band_alpha`,
            // with the `▌` bar's accent likewise faded against that fill —
            // the bar keeps its half-cell width through the whole fade.
            let row_fill = if let Some(a) = band_alpha {
                let base = Self::resolve_fill(theme, RowHighlight::None, theme.panel, hover_t);
                let blended = crate::theme::mix(base, theme.selection, a);
                fill(
                    buf,
                    Rect::new(list_area.x, text_row, list_area.width, 1),
                    blended,
                );
                if let Some(cell) = buf.cell_mut((list_area.x, text_row)) {
                    cell.set_symbol("\u{258c}");
                    cell.set_fg(crate::theme::mix(blended, theme.accent, a));
                }
                blended
            } else {
                ListRow {
                    highlight,
                    zebra: None,
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
                if lifted {
                    let lift = crate::theme::mix(theme.selection, theme.control_hover, 0.5);
                    // Keep the `▌` bar cell: refill only the row's body.
                    fill(
                        buf,
                        Rect::new(list_area.x + 1, text_row, list_area.width.saturating_sub(1), 1),
                        lift,
                    );
                    lift
                } else {
                    Self::resolve_fill(theme, highlight, theme.panel, hover_t)
                }
            };

            self.paint_row(
                buf, row, text_row, list_area, row_fill, is_open, is_dragged, theme,
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

    /// Replicates [`ListRow::paint`]'s own fill computation, so a row
    /// painted through it can be asked separately what color its content
    /// (text) should sit on — needed because `paint_row` sits *outside*
    /// `ListRow`, painted as a second pass on top of it.
    fn resolve_fill(theme: &Theme, highlight: RowHighlight, base: Color, hover_t: f32) -> Color {
        match highlight {
            RowHighlight::None => base,
            RowHighlight::Hover => crate::theme::mix(base, theme.control, hover_t),
            RowHighlight::Cursor => theme.control_hover,
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
    /// name); the selection band is that row's real marker, so the name
    /// itself keeps the normal text color.
    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        buf: &mut Buffer,
        row: &Row,
        text_row: u16,
        list_area: Rect,
        row_fill: Color,
        open: bool,
        dragged: bool,
        theme: &Theme,
    ) {
        let right = list_area.x + list_area.width;
        // Column 0 is reserved for the selection's accent bar (painted by
        // `ListRow` or the travel band), whether or not this row is
        // selected, so content never shifts when selection changes.
        let text_x = list_area.x + 1;

        // A row being dragged paints its grip glyph in the reserved
        // accent-bar column — nothing else shifts.
        if dragged {
            text(
                buf,
                list_area.x,
                text_row,
                "\u{22ee}",
                theme.accent,
                row_fill,
                false,
            );
        }

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

                // A send still waiting on its result marks its row with a
                // pending glyph, whichever request is open on screen.
                let content_x = if self.in_flight.contains(slug.as_str()) {
                    text(
                        buf,
                        content_x,
                        text_row,
                        "\u{21bb} ",
                        theme.accent,
                        row_fill,
                        false,
                    );
                    content_x + 2
                } else {
                    content_x
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

    fn req_row(slug: &str) -> Row {
        Row::Request { slug: slug.into(), name: slug.into(), depth: 0, broken: None, method: None }
    }

    /// Spec §4.5: the verbs are the router's rows now; the sidebar's own
    /// keys are named keys only.
    #[test]
    fn the_sidebar_has_no_verb_letters() {
        let mut s = Sidebar { rows: vec![req_row("main/a")], selected: Some(0), ..Sidebar::default() };
        for c in ['n', 'a', 'r', 'd', 'm', 'y', 'p', 'P'] {
            assert_eq!(s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)), None, "{c}");
        }
    }

    /// Spec §5.2: N request rows from the cursor down, folder rows
    /// skipped, a count past the end clamped, and nothing on a folder row.
    #[test]
    fn request_slugs_from_clamps_and_skips_folders() {
        let s = Sidebar {
            rows: vec![
                req_row("main/a"),
                req_row("main/b"),
                Row::Folder { path: "main/f".into(), name: "f".into(), depth: 0, expanded: true },
                req_row("main/f/c"),
                req_row("main/d"),
            ],
            ..Sidebar::default()
        };
        assert_eq!(s.request_slugs_from(1, 2), ["main/b", "main/f/c"], "the folder row is skipped");
        assert_eq!(s.request_slugs_from(3, 9), ["main/f/c", "main/d"], "clamped at the end");
        assert!(s.request_slugs_from(2, 3).is_empty(), "dd on a folder does nothing");
        assert!(s.request_slugs_from(9, 1).is_empty(), "no row there");
    }

    /// Spec §5.2: a put anchors only on a real request slug: the request
    /// row itself, or a visible request inside the folder row. A collapsed
    /// or empty folder, and no selection, give nothing.
    #[test]
    fn put_anchor_is_always_a_request_slug() {
        let mut s = Sidebar {
            rows: vec![
                req_row("main/a"),
                Row::Folder { path: "main/f".into(), name: "f".into(), depth: 0, expanded: true },
                req_row("main/f/c"),
                req_row("main/f/d"),
                Row::Folder { path: "main/g".into(), name: "g".into(), depth: 0, expanded: false },
                req_row("main/z"),
                // A folder holding only an expanded subfolder, and one
                // holding direct requests around a subfolder.
                Row::Folder { path: "main/h".into(), name: "h".into(), depth: 0, expanded: true },
                Row::Folder { path: "main/h/i".into(), name: "i".into(), depth: 1, expanded: true },
                req_row("main/h/i/e"),
                Row::Folder { path: "main/j".into(), name: "j".into(), depth: 0, expanded: true },
                req_row("main/j/k"),
                Row::Folder { path: "main/j/m".into(), name: "m".into(), depth: 1, expanded: true },
                req_row("main/j/m/n"),
                req_row("main/j/o"),
            ],
            ..Sidebar::default()
        };
        assert_eq!(s.put_anchor(false), None, "no selection");
        s.selected = Some(0);
        assert_eq!(s.put_anchor(false).as_deref(), Some("main/a"));
        assert_eq!(s.put_anchor(true).as_deref(), Some("main/a"));
        s.selected = Some(1);
        assert_eq!(s.put_anchor(false).as_deref(), Some("main/f/d"), "below: the last inside");
        assert_eq!(s.put_anchor(true).as_deref(), Some("main/f/c"), "above: the first inside");
        s.selected = Some(4);
        assert_eq!(s.put_anchor(false), None, "a collapsed folder has nothing visible inside");
        assert_eq!(s.put_anchor(true), None);
        assert!(s.selected_is_folder());
        s.selected = Some(6);
        assert_eq!(s.put_anchor(false), None, "only a subfolder: no direct request");
        assert_eq!(s.put_anchor(true), None);
        assert!(s.selected_is_folder());
        s.selected = Some(9);
        assert_eq!(s.put_anchor(false).as_deref(), Some("main/j/o"), "never the subfolder's request");
        assert_eq!(s.put_anchor(true).as_deref(), Some("main/j/k"));
        s.selected = Some(0);
        assert!(!s.selected_is_folder());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn listing(slugs: &[&str]) -> Vec<RequestListing> {
        slugs
            .iter()
            .map(|s| RequestListing {
                name: None,
                slug: format!("main/{s}"),
                broken: None,
                method: Some(Method::Get),
            })
            .collect()
    }

    fn expanded(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| format!("main/{s}")).collect()
    }

    #[test]
    fn rows_show_display_names_sorted_by_name_not_slug() {
        let mut s = Sidebar::default();
        let mut l = listing(&["zeta-slug", "alpha-slug", "legacy"]);
        // Display names deliberately invert the slug order.
        l[0].name = Some("Aardvark".into()); // zeta-slug
        l[1].name = Some("Zebra".into()); // alpha-slug
        // l[2] "legacy" stays nameless -> displays its slug leaf.
        s.refresh(l, "main", &expanded(&[]), &[]);
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

    fn order(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn row_slugs(s: &Sidebar) -> Vec<String> {
        s.rows
            .iter()
            .map(|r| match r {
                Row::Request { slug, .. } => slug.clone(),
                Row::Folder { path, .. } => format!("{path}/"),
            })
            .collect()
    }

    #[test]
    fn rows_honour_the_order_list_per_level_and_folders_stay_alphabetical() {
        let mut s = Sidebar::default();
        let l = listing(&["a", "b", "c", "zed/x", "zed/y", "auth/p"]);
        s.refresh(
            l,
            "main",
            &expanded(&["zed", "auth"]),
            &order(&["c", "a", "zed/y"]),
        );
        assert_eq!(
            row_slugs(&s),
            [
                "main/c",
                "main/a",
                "main/b", // listed first, then b
                "main/auth/",
                "main/auth/p", // folders alphabetical below
                "main/zed/",
                "main/zed/y",
                "main/zed/x",
            ]
        );
    }

    #[test]
    fn selection_survives_a_reorder_rebuild_by_identity() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), "main", &expanded(&[]), &[]);
        s.selected = Some(2); // main/c
        s.rebuild("main", &expanded(&[]), &order(&["c", "a", "b"]));
        assert_eq!(s.selected, Some(0));
        assert_eq!(s.selected_slug().as_deref(), Some("main/c"));
    }

    #[test]
    fn tree_builds_nested_folders_and_hides_collapsed_children() {
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["api/users/list", "api/users/create", "api/ping", "top"]),
            "main",
            &expanded(&[]),
            &[],
        );
        // collapsed: only top-level rows visible
        assert_eq!(
            s.rows,
            vec![
                Row::Request {
                    slug: "main/top".into(),
                    name: "top".into(),
                    depth: 0,
                    broken: None,
                    method: Some(Method::Get),
                },
                Row::Folder {
                    path: "main/api".into(),
                    name: "api".into(),
                    depth: 0,
                    expanded: false
                },
            ]
        );
        s.refresh(
            listing(&["api/users/list", "api/users/create", "api/ping", "top"]),
            "main",
            &expanded(&["api"]),
            &[],
        );
        assert_eq!(
            s.rows,
            vec![
                Row::Request {
                    slug: "main/top".into(),
                    name: "top".into(),
                    depth: 0,
                    broken: None,
                    method: Some(Method::Get),
                },
                Row::Folder {
                    path: "main/api".into(),
                    name: "api".into(),
                    depth: 0,
                    expanded: true
                },
                Row::Request {
                    slug: "main/api/ping".into(),
                    name: "ping".into(),
                    depth: 1,
                    broken: None,
                    method: Some(Method::Get),
                },
                Row::Folder {
                    path: "main/api/users".into(),
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
        s.refresh(listing(&["api/ping", "top"]), "main", &expanded(&[]), &[]);
        s.handle_key(key(KeyCode::Down)); // first press lands on row 0 ("top")
        s.handle_key(key(KeyCode::Down)); // now on the "api" folder row
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

    /// The ctrl chords land in the same state as the named page keys they
    /// stand for, from the same start; the vim letters are not the
    /// sidebar's (piece 4 §4.5: the router replays them as named keys).
    #[test]
    fn page_chords_are_strict_synonyms_and_the_vim_letters_are_gone() {
        fn fresh() -> Sidebar {
            let mut s = Sidebar::default();
            s.refresh(listing(&["api/a", "api/b", "c", "d", "e", "f"]), "main", &expanded(&["api"]), &[]);
            s.handle_key(key(KeyCode::Down));
            s.handle_key(key(KeyCode::Down));
            s.last_list_height = 4;
            s
        }
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        for letter in [
            key(KeyCode::Char('j')),
            key(KeyCode::Char('k')),
            key(KeyCode::Char('h')),
            key(KeyCode::Char('l')),
            key(KeyCode::Char('g')),
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        ] {
            let mut s = fresh();
            let before = s.selected;
            assert_eq!(s.handle_key(letter), None, "{letter:?}");
            assert_eq!(s.selected, before, "{letter:?} moved the sidebar");
        }
        let pairs: [(KeyEvent, KeyEvent); 2] = [
            (ctrl('f'), key(KeyCode::PageDown)),
            (ctrl('b'), key(KeyCode::PageUp)),
        ];
        for (alias, canonical) in pairs {
            let (mut a, mut b) = (fresh(), fresh());
            let ra = a.handle_key(alias);
            let rb = b.handle_key(canonical);
            assert_eq!(ra, rb, "{alias:?} vs {canonical:?}: action");
            assert_eq!(a.selected, b.selected, "{alias:?} vs {canonical:?}: selection");
        }
        // Full pages: ctrl+f/ctrl+b (PageDown/PageUp) move by the list
        // height; ctrl+d/ctrl+u by half of it. `fresh()` starts on row 0
        // (R6: the cursor is always somewhere) and its two `j` presses land
        // it on row 2, so these are relative to that, not an absolute 1.
        let mut s = fresh();
        let start = s.selected.expect("fresh() lands on a row");
        s.handle_key(ctrl('f'));
        assert_eq!(s.selected, Some(6), "clamped at the last row");
        s.handle_key(ctrl('b'));
        assert_eq!(s.selected, Some(2), "a full page back from the last row");
        s.handle_key(ctrl('d'));
        assert_eq!(
            s.selected,
            Some(start + 2),
            "half of the 4-row page, from fresh()'s row 2"
        );
        s.handle_key(ctrl('u'));
        assert_eq!(s.selected, Some(start), "half a page back lands where fresh() started");
    }

    #[test]
    fn wheel_scroll_is_free_and_keyboard_still_tracks_selection() {
        let mut s = Sidebar::default();
        let slugs: Vec<String> = (0..30).map(|i| format!("r{i:02}")).collect();
        let refs: Vec<&str> = slugs.iter().map(|s| s.as_str()).collect();
        s.refresh(listing(&refs), "main", &expanded(&[]), &[]);
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
        s.handle_key(key(KeyCode::Down));
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
        s.refresh(listing(&["a/b/c"]), "main", &expanded(&[]), &[]);
        s.select_slug("main/a/b/c");
        assert!(s.pending_expand.contains("main/a") && s.pending_expand.contains("main/a/b"));
    }

    /// Only folder rows' paths are queued, never the space: the tree is
    /// rooted at `space/`, so the space is no row, and queuing it wrote a
    /// meaningless `expanded` entry an undo could not take back. A loose
    /// slug (no space; core never lists one) has no folder to open.
    #[test]
    fn expand_to_queues_folder_paths_only_not_the_space() {
        let queued = |slug: &str| {
            let mut s = Sidebar::default();
            s.expand_to(slug);
            s.pending_expand.into_iter().collect::<Vec<_>>()
        };
        assert_eq!(queued("main/f/x"), ["main/f"]);
        assert_eq!(queued("main/f/g/x"), ["main/f", "main/f/g"]);
        assert!(queued("main/alpha").is_empty());
        assert!(queued("ping").is_empty());
    }

    #[test]
    fn a_rebuild_whose_cursor_row_vanished_lands_on_the_neighbour_request() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["alpha", "beta", "gamma"]), "main", &expanded(&[]), &[]);
        s.select_slug("main/beta");
        s.refresh(listing(&["alpha", "gamma"]), "main", &expanded(&[]), &[]);
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("main/gamma"),
            "the row that slid up"
        );
        s.refresh(listing(&["alpha"]), "main", &expanded(&[]), &[]);
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("main/alpha"),
            "the last row went: the one above"
        );
    }

    #[test]
    fn a_rebuild_with_rows_never_leaves_the_cursor_empty() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["alpha", "sub/x"]), "main", &expanded(&[]), &[]);
        assert_eq!(
            s.selected_slug().as_deref(),
            Some("main/alpha"),
            "a fresh list starts on its first request"
        );
        s.refresh(listing(&["sub/x"]), "main", &expanded(&[]), &[]);
        assert_eq!(
            s.selected,
            Some(0),
            "only a folder row left: the cursor sits on it"
        );
        s.refresh(listing(&[]), "main", &expanded(&[]), &[]);
        assert_eq!(s.selected, None, "no rows, no cursor");
    }

    #[test]
    fn first_request_in_space_looks_inside_collapsed_folders_and_prefers_the_top_level() {
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["deploy/run", "deploy/prod/x"]),
            "main",
            &expanded(&[]),
            &[],
        );
        assert_eq!(
            s.first_visible_request_in_space("main", &[], &expanded(&[])),
            None
        );
        assert_eq!(
            s.first_request_in_space("main", &[]).as_deref(),
            Some("main/deploy/run")
        );
        s.refresh(
            listing(&["deploy/run", "top"]),
            "main",
            &expanded(&[]),
            &[],
        );
        assert_eq!(
            s.first_request_in_space("main", &[]).as_deref(),
            Some("main/top")
        );
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
            pointer: None,
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
        s.refresh(listing(&refs), "main", &expanded(&[]), &[]);

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
        // 12-row pane: button(3) + spacer(1) = 4 rows overhead -- the top
        // cap is the padding that used to be a blank row -- leaving 8
        // lines for the list, so 8 rows fit on the dense 1-line pitch.
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
        s.refresh(listing(&["a", "b", "c"]), "main", &expanded(&[]), &[]);
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
        s.refresh(
            listing(&["api/ping", "top"]),
            "main",
            &expanded(&["api"]),
            &[],
        );
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
            button_rect.y, 0,
            "the button starts on the pane's first row -- its top cap is \
             the padding that used to be a blank row"
        );
        assert_eq!(button_rect.height, 3, "the paint-layer button is 3 rows");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(button_rect.x, button_rect.y + 2)].symbol(),
            crate::paint::cap::CAP_BOTTOM,
            "button's bottom row is its cap"
        );
        assert_eq!(
            buf[(button_rect.x, button_rect.y + 2)].fg,
            theme.panel,
            "the sidebar sits on `panel`, so its button must cap against \
             `panel` -- a cap that guessed `page` would fringe the button"
        );

        // rows[0] = "top", rows[1] = folder "api" (expanded), rows[2] = "api/ping"
        // list starts at y = button(3) + spacer(1) = 4.
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).expect("row 0 hit");
        assert_eq!(row0.y, 4, "row 0 sits right at the list top, 1-line pitch");
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
        s.refresh(listing(&["top", "next"]), "main", &expanded(&[]), &[]);
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

    /// The OPEN request's row keeps the full `▌` bar + `theme.selection`
    /// fill — the band always means "this is what the right panes show".
    /// The keyboard cursor, resting on a *different* row, gets only a
    /// steady `theme.control_hover` fill: visibly targeted,
    /// clearly not the band.
    #[test]
    fn open_row_and_cursor_row_stay_visually_distinct() {
        let mut s = Sidebar::default();
        // Rows sort by slug: row 0 is "next", row 1 is "top".
        s.refresh(listing(&["top", "next"]), "main", &expanded(&[]), &[]);
        s.open_slug = Some("main/next".into());
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

        // Row 0 ("next", open, not cursor): the `▌` bar + selection fill,
        // name in the normal text color — the band itself is the marker.
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let bar_cell = buf[(row0.x, row0.y)].clone();
        let fill_cell = buf[(row0.x + row0.width - 2, row0.y)].clone();
        let name_cell = buf[(row0.x + 7, row0.y)].clone();
        assert_eq!(bar_cell.symbol(), "\u{258c}", "accent bar on the open row");
        assert_eq!(bar_cell.fg, theme.accent);
        assert_eq!(fill_cell.bg, theme.selection);
        assert_eq!(
            name_cell.symbol(),
            "n",
            "sanity: sampled the name cell (\"next\")"
        );
        assert_eq!(
            name_cell.fg, theme.text,
            "open row's name stays normal-colored; the band carries the signal"
        );

        // Row 1 ("top", cursor, not open): no bar, no selection fill —
        // a steady `theme.control` fill marks the cursor instead.
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        let bar_cell = buf[(row1.x, row1.y)].clone();
        let fill_cell = buf[(row1.x + row1.width - 2, row1.y)].clone();
        let name_cell = buf[(row1.x + 7, row1.y)].clone();
        assert_ne!(
            bar_cell.symbol(),
            "\u{258c}",
            "no accent bar on the cursor row"
        );
        assert_eq!(
            fill_cell.bg, theme.control_hover,
            "cursor row carries the steady control_hover fill"
        );
        assert_eq!(
            name_cell.symbol(),
            "t",
            "sanity: sampled \"top\"'s name cell"
        );
        assert_eq!(
            name_cell.fg, theme.text,
            "cursor row's name stays normal-colored"
        );
    }

    /// The band ignores pane focus: the open request keeps its bar + fill
    /// even while another pane is active (only the cursor marker is
    /// focus-gated).
    #[test]
    fn open_row_keeps_its_band_when_the_pane_is_unfocused() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["top", "next"]), "main", &expanded(&[]), &[]);
        s.open_slug = Some("main/next".into());
        s.selected = Some(1); // cursor elsewhere
        let theme = Theme::dark();
        let mut ctx = draw_ctx(&theme, None);
        ctx.focused = false;
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();
        let buf = terminal.backend().buffer();

        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        assert_eq!(
            buf[(row0.x, row0.y)].symbol(),
            "\u{258c}",
            "open row's bar survives losing pane focus"
        );
        assert_eq!(buf[(row0.x + row0.width - 2, row0.y)].bg, theme.selection);

        // The unfocused cursor row shows no marker of its own.
        let row1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        assert_ne!(
            buf[(row1.x + row1.width - 2, row1.y)].bg,
            theme.control_hover
        );
        assert_ne!(buf[(row1.x + row1.width - 2, row1.y)].bg, theme.selection);
    }

    /// When the cursor lands on the open request itself (the common case
    /// right after Enter), the band's fill lifts half a step toward the
    /// cursor fill — a cue of its own — while the name stays plain, no
    /// separate open-accent styling layered underneath it.
    #[test]
    fn cursor_on_the_open_row_lifts_the_selection_fill_and_keeps_the_name_plain() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["only"]), "main", &expanded(&[]), &[]);
        s.open_slug = Some("main/only".into());
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
        assert_eq!(
            fill_cell.bg,
            crate::theme::mix(theme.selection, theme.control_hover, 0.5)
        );
        assert_eq!(
            name_cell.fg, theme.text,
            "coinciding open+cursor: the lifted fill wins, name stays normal-colored"
        );
    }

    #[test]
    fn the_open_row_lifts_while_the_focused_cursor_is_on_it() {
        let paint = |focused: bool, cursor: usize| {
            let mut s = Sidebar::default();
            s.refresh(listing(&["a", "b"]), "main", &expanded(&[]), &[]);
            s.open_slug = Some("main/a".into());
            s.selected = Some(cursor);
            let theme = Theme::dark();
            let mut ctx = draw_ctx(&theme, None);
            ctx.focused = focused;
            let backend = ratatui::backend::TestBackend::new(30, 12);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let mut hits = HitMap::default();
            terminal.draw(|f| s.draw(f, f.area(), &ctx, &mut hits)).unwrap();
            let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
            terminal.backend().buffer()[(row0.x + row0.width - 2, row0.y)].bg
        };
        let theme = Theme::dark();
        assert_eq!(
            paint(true, 0),
            crate::theme::mix(theme.selection, theme.control_hover, 0.5),
            "focused + on the open row: lifted"
        );
        assert_eq!(paint(false, 0), theme.selection, "unfocused: as today");
        assert_eq!(
            paint(true, 1),
            theme.selection,
            "cursor elsewhere: the open row as today"
        );
    }

    #[test]
    fn request_row_paints_a_method_tag() {
        let mut s = Sidebar::default();
        // Two rows, neither selected/hovered/open: both sit on the flat
        // panel fill (the request list keeps no zebra striping) — check
        // the tag's colors against the surface it actually sits on.
        s.refresh(
            vec![
                RequestListing {
                    name: None,
                    slug: "main/ping".into(),
                    broken: None,
                    method: Some(Method::Get),
                },
                RequestListing {
                    name: None,
                    slug: "main/pong".into(),
                    broken: None,
                    method: Some(Method::Get),
                },
            ],
            "main",
            &expanded(&[]),
            &[],
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
            tag_cell.bg, theme.panel,
            "tag sits directly on the row's flat panel fill"
        );
    }

    /// A request with a send still waiting shows a pending glyph between
    /// its method tag and name — background sends stay visible while the
    /// user works elsewhere.
    #[test]
    fn in_flight_request_row_paints_a_pending_glyph() {
        let mut s = Sidebar::default();
        s.refresh(
            vec![RequestListing {
                name: None,
                slug: "main/ping".into(),
                broken: None,
                method: Some(Method::Get),
            }],
            "main",
            &expanded(&[]),
            &[],
        );
        s.in_flight.insert("main/ping".into());
        let (_, hits) = render_hits(&mut s);
        let row0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let theme = Theme::dark();
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let ctx = draw_ctx(&theme, None);
        let mut hits2 = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits2))
            .unwrap();
        let buf = terminal.backend().buffer();
        // Glyph sits where the name would otherwise start: one past the
        // accent-bar column plus the padded tag field.
        let glyph = buf[(row0.x + 1 + TAG_WIDTH + 1, row0.y)].clone();
        assert_eq!(glyph.symbol(), "\u{21bb}");
        assert_eq!(glyph.fg, theme.accent);
        let name = buf[(row0.x + 1 + TAG_WIDTH + 3, row0.y)].clone();
        assert_eq!(name.symbol(), "p", "name follows the glyph");

        s.in_flight.clear();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut HitMap::default()))
            .unwrap();
        let cleared = terminal.backend().buffer()[(row0.x + 1 + TAG_WIDTH + 1, row0.y)].clone();
        assert_eq!(cleared.symbol(), "p", "glyph gone once the send settles");
    }

    #[test]
    fn broken_request_row_has_no_chip() {
        let mut s = Sidebar::default();
        s.refresh(
            vec![RequestListing {
                name: None,
                slug: "main/bad".into(),
                broken: Some("parse error".into()),
                method: None,
            }],
            "main",
            &expanded(&[]),
            &[],
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
    fn consecutive_rows_get_adjacent_1_line_hit_rects() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), "main", &expanded(&[]), &[]);
        let (_, hits) = render_hits(&mut s);
        let r0 = hits.rect_of(&Hit::SidebarRow(0)).unwrap();
        let r1 = hits.rect_of(&Hit::SidebarRow(1)).unwrap();
        let r2 = hits.rect_of(&Hit::SidebarRow(2)).unwrap();
        assert_eq!(r0.height, 1);
        assert_eq!(r1.y, r0.y + 1, "rows are exactly adjacent, no gap");
        assert_eq!(r2.y, r1.y + 1);
    }

    /// Mid-flight, the band CROSSFADES between the previously open row and
    /// the newly open one — both rows carry a partial-strength selection
    /// fill and a half-width `▌` bar at partial accent — rather than a
    /// sliding fractional-height band (whose full-cell edge fills used to
    /// make the bar read as doubling in width).
    #[test]
    fn mid_flight_band_crossfades_between_the_old_and_new_open_rows() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), "main", &expanded(&[]), &[]);
        // The band is moving from row 0 ("a", previously open) toward the
        // newly open row 1 ("b").
        s.open_slug = Some("main/b".into());
        s.band_fade_from = Some(0);
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
            pointer: None,
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

        // Both crossfading rows keep the half-width bar glyph — never a
        // fractional-height block or a flooded full cell.
        for r in [&row0, &row1] {
            assert_eq!(
                buf[(r.x, r.y)].symbol(),
                "\u{258c}",
                "crossfading rows keep the half-width bar"
            );
        }
        // And their fills sit strictly between the resting surface and the
        // full selection color: partial strength on both ends of the fade.
        for r in [&row0, &row1] {
            let bg = buf[(r.x + r.width - 2, r.y)].bg;
            assert_ne!(bg, theme.selection, "mid-fade fill is not full strength");
            assert_ne!(bg, theme.panel, "mid-fade fill is not the resting surface");
        }
        // No fractional-height glyphs anywhere in the list's bar column
        // (three rows: "a", "b", "c").
        for y in row0.y..row0.y + 3 {
            let sym = buf[(row0.x, y)].symbol();
            assert!(
                sym == " " || sym == "\u{258c}",
                "bar column must hold only spaces or the half-width bar, got {sym:?} at y={y}"
            );
        }
    }

    #[test]
    fn group_bounds_covers_one_levels_requests() {
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["a", "b", "auth/x", "auth/y", "auth/deep/z"]),
            "main",
            &expanded(&["auth", "auth/deep"]),
            &[],
        );
        // rows: a b auth/ x y deep/ z
        assert_eq!(s.group_bounds(0), Some((0, 1)));
        assert_eq!(s.group_bounds(1), Some((0, 1)));
        assert_eq!(s.group_bounds(2), None, "a folder row has no group");
        assert_eq!(s.group_bounds(3), Some((3, 4)));
        assert_eq!(s.group_bounds(6), Some((6, 6)));
    }

    #[test]
    fn group_bounds_stops_at_a_folder_row_between_equal_depths() {
        // Two sibling subtrees at the *same* depth under different
        // folders: depth alone would run them together, so the folder row
        // sitting between them has to break the run.
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["auth/x", "auth/y", "billing/z"]),
            "main",
            &expanded(&["auth", "billing"]),
            &[],
        );
        // rows: auth/ x y billing/ z — x, y and z are all depth 1.
        assert_eq!(row_slugs(&s).len(), 5);
        assert_eq!(s.group_bounds(1), Some((1, 2)), "auth's two requests");
        assert_eq!(s.group_bounds(2), Some((1, 2)));
        assert_eq!(s.group_bounds(3), None, "billing's folder row");
        assert_eq!(
            s.group_bounds(4),
            Some((4, 4)),
            "billing's one request is its own group, not auth's"
        );
    }

    #[test]
    fn a_working_drag_order_overrides_exactly_one_level() {
        let mut s = Sidebar::default();
        s.refresh(
            listing(&["a", "b", "c", "auth/x", "auth/y"]),
            "main",
            &expanded(&["auth"]),
            &[],
        );
        assert!(s.begin_drag(2, "main")); // main/c
        let d = s.drag.as_ref().unwrap();
        assert_eq!(d.original, ["a", "b", "c"]);
        assert_eq!(d.level, "");
        assert!(s.drag_to_row(0));
        s.rebuild("main", &expanded(&["auth"]), &[]);
        assert_eq!(
            row_slugs(&s),
            [
                "main/c",
                "main/a",
                "main/b",
                "main/auth/",
                "main/auth/x",
                "main/auth/y"
            ]
        );
        // Pointer past the group's end pins to the last slot.
        assert!(s.drag_to_row(10));
        s.rebuild("main", &expanded(&["auth"]), &[]);
        assert_eq!(row_slugs(&s)[..3], ["main/a", "main/b", "main/c"]);
        assert!(!s.drag_to_row(10), "no change reports false");
        s.drag = None;
        s.rebuild("main", &expanded(&["auth"]), &[]);
        assert_eq!(row_slugs(&s)[..3], ["main/a", "main/b", "main/c"]);
    }

    #[test]
    fn drag_to_row_finds_the_dragged_request_by_slug_not_row_offset() {
        // The listing changes under a live drag (a reload rebuilt the rows
        // with one sibling gone): the dragged request is still the one
        // that moves, and the working order is never touched by offset.
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c", "d"]), "main", &expanded(&[]), &[]);
        assert!(s.begin_drag(3, "main"));
        s.refresh(listing(&["a", "c", "d"]), "main", &expanded(&[]), &[]);
        assert!(s.drag_to_row(0));
        assert_eq!(
            s.drag.as_ref().unwrap().working,
            order(&["d", "a", "b", "c"])
        );
    }

    #[test]
    fn begin_drag_refuses_a_folder_row() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "auth/x"]), "main", &expanded(&[]), &[]);
        assert!(!s.begin_drag(1, "main"));
        assert!(s.drag.is_none());
    }

    #[test]
    fn draw_paints_the_grip_glyph_only_on_the_dragged_row() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b", "c"]), "main", &expanded(&[]), &[]);
        assert!(s.begin_drag(0, "main")); // main/a
        let theme = Theme::dark();
        let ctx = draw_ctx(&theme, None);
        let backend = ratatui::backend::TestBackend::new(30, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| s.draw(f, f.area(), &ctx, &mut hits))
            .unwrap();

        let dragged_row = hits.rect_of(&Hit::SidebarRow(0)).expect("dragged row hit");
        let other_row = hits.rect_of(&Hit::SidebarRow(1)).expect("other row hit");
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(dragged_row.x, dragged_row.y)].symbol(),
            "\u{22ee}",
            "dragged row's gutter shows the grip glyph"
        );
        assert_ne!(
            buf[(other_row.x, other_row.y)].symbol(),
            "\u{22ee}",
            "non-dragged rows never show the grip glyph"
        );
    }

    #[test]
    fn alt_up_and_down_dispatch_move_request_for_the_selected_row() {
        let mut s = Sidebar::default();
        s.refresh(listing(&["a", "b"]), "main", &expanded(&[]), &[]);
        s.selected = Some(1);
        let ev = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        assert_eq!(
            s.handle_key(ev),
            Some(Action::MoveRequest {
                slug: "main/b".into(),
                delta: -1
            })
        );
        let ev = KeyEvent::new(KeyCode::Down, KeyModifiers::ALT);
        assert_eq!(
            s.handle_key(ev),
            Some(Action::MoveRequest {
                slug: "main/b".into(),
                delta: 1
            })
        );
        // Plain Up still just moves the cursor.
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Action::Render)
        );
        assert_eq!(s.selected, Some(0));
    }

    /// A key of the fold probes (plan M5b Task 3).
    #[derive(Debug, Clone, Copy)]
    enum FoldKey {
        Z(crate::action::FoldOp, usize),
        Down,
        Up,
    }

    /// The fold probes' tree: `top`, then `api/` (`ping`, `pong`, `auth/`
    /// (`login`)), then `zeta/` (`z1`), with `open` folders and the cursor
    /// on `at`. Its rows are the probe buffer's lines, in order.
    fn probe_tree(open: &[&str], at: &RowKey) -> (Sidebar, BTreeSet<String>) {
        let mut s = Sidebar::default();
        let open = expanded(open);
        s.refresh(listing(&["top", "api/ping", "api/pong", "api/auth/login", "zeta/z1"]), "main", &open, &[]);
        assert!(s.select_key(at), "{at:?} shows");
        (s, open)
    }

    /// One key as the app runs it (`Action::SidebarFold`): the new folders
    /// kept and the rows rebuilt, then the cursor put back. Whether it was
    /// Vim's "No fold found".
    fn press(s: &mut Sidebar, open: &mut BTreeSet<String>, key: FoldKey) -> bool {
        let (op, count) = match key {
            FoldKey::Down => {
                s.move_selection(1);
                return false;
            }
            FoldKey::Up => {
                s.move_selection(-1);
                return false;
            }
            FoldKey::Z(op, count) => (op, count),
        };
        let real = s.fold_cursor().expect("a cursor");
        match s.fold_change(op, count, Some(&real), open, "main") {
            FoldChange::NoFold => return true,
            FoldChange::Same => {}
            FoldChange::Expanded(set) => {
                *open = set;
                s.rebuild("main", open, &[]);
                s.show_fold_cursor(real);
            }
        }
        false
    }

    /// Task 3 review: a request neither shown nor under a closed folder (it
    /// was deleted from outside) is no longer remembered as the hidden row.
    #[test]
    fn a_hidden_request_that_vanished_is_forgotten() {
        let (mut s, mut open) = probe_tree(&["api", "api/auth", "zeta"], &RowKey::Request("main/api/pong".into()));
        press(&mut s, &mut open, FoldKey::Z(crate::action::FoldOp::Close, 1));
        let folder = RowKey::Folder("main/api".into());
        assert_eq!(s.selected_key(), Some(folder.clone()));
        assert_eq!(s.fold_cursor(), Some(RowKey::Request("main/api/pong".into())), "remembered while it exists");
        s.show_fold_cursor(RowKey::Request("main/zeta/gone".into()));
        assert_eq!(s.fold_cursor(), Some(folder), "the vanished row is not remembered");
    }

    /// One fold probe: the folders open before, the cursor, the keys, the
    /// folders shut after (a folder shut inside a shut one included), the
    /// row the cursor is really on, the row it is drawn on, and whether a
    /// key said "No fold found".
    struct FoldCase<'a> {
        id: &'a str,
        open: &'a [&'a str],
        at: RowKey,
        keys: Vec<FoldKey>,
        closed: &'a [&'a str],
        real: RowKey,
        shown: RowKey,
        no_fold: bool,
    }

    /// Plan M5b Deviations 9–11: every fold key against Vim 9.1 (the probe
    /// record's F1–F38, lines as rows). `closed` lists the folders shut
    /// after the keys, a folder shut inside a shut one included; `real` is
    /// the row the cursor is on, `shown` the row it is drawn on.
    #[test]
    fn the_sidebar_folds_as_vim_folds() {
        use crate::action::FoldOp::{Close, CloseAll, Open, OpenAll, Toggle};
        use FoldKey::{Down, Up, Z};
        let req = |s: &str| RowKey::Request(format!("main/{s}"));
        let dir = |s: &str| RowKey::Folder(format!("main/{s}"));
        let all = ["api", "api/auth", "zeta"];
        let zc = Z(Close, 1);
        let zo = Z(Open, 1);
        let za = Z(Toggle, 1);
        let zm = Z(CloseAll, 1);
        let zr = Z(OpenAll, 1);
        let cases = vec![
            FoldCase { id: "F1", open: &all, at: req("api/pong"), keys: vec![zc], closed: &["api"], real: req("api/pong"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F2", open: &all, at: req("api/auth/login"), keys: vec![zc], closed: &["api/auth"], real: req("api/auth/login"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F3", open: &all, at: req("api/auth/login"), keys: vec![zc, zc], closed: &["api", "api/auth"], real: req("api/auth/login"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F4", open: &all, at: req("api/auth/login"), keys: vec![zc, zc, zc], closed: &["api", "api/auth"], real: req("api/auth/login"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F5", open: &all, at: dir("api"), keys: vec![zc], closed: &["api"], real: dir("api"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F6", open: &all, at: dir("api"), keys: vec![zc, zc], closed: &["api"], real: dir("api"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F7", open: &all, at: req("top"), keys: vec![zc], closed: &[], real: req("top"), shown: req("top"), no_fold: true },
            FoldCase { id: "F8", open: &all, at: req("top"), keys: vec![zo], closed: &[], real: req("top"), shown: req("top"), no_fold: true },
            FoldCase { id: "F9", open: &all, at: req("top"), keys: vec![za], closed: &[], real: req("top"), shown: req("top"), no_fold: true },
            FoldCase { id: "F10", open: &all, at: req("api/ping"), keys: vec![zo], closed: &[], real: req("api/ping"), shown: req("api/ping"), no_fold: false },
            FoldCase { id: "F11", open: &all, at: req("api/auth/login"), keys: vec![zm, zo], closed: &["api/auth", "zeta"], real: req("api/auth/login"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F12", open: &all, at: req("api/auth/login"), keys: vec![zm, Z(Open, 2)], closed: &["zeta"], real: req("api/auth/login"), shown: req("api/auth/login"), no_fold: false },
            FoldCase { id: "F13", open: &all, at: req("api/auth/login"), keys: vec![zm, zr], closed: &[], real: req("api/auth/login"), shown: req("api/auth/login"), no_fold: false },
            FoldCase { id: "F14a", open: &all, at: req("api/auth/login"), keys: vec![za], closed: &["api/auth"], real: req("api/auth/login"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F14b", open: &all, at: req("api/auth/login"), keys: vec![za, za], closed: &[], real: req("api/auth/login"), shown: req("api/auth/login"), no_fold: false },
            FoldCase { id: "F15", open: &all, at: req("api/auth/login"), keys: vec![Z(Toggle, 2)], closed: &["api", "api/auth"], real: req("api/auth/login"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F16", open: &all, at: req("api/auth/login"), keys: vec![zm, za], closed: &["api/auth", "zeta"], real: req("api/auth/login"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F17", open: &all, at: req("api/auth/login"), keys: vec![zm, Z(Toggle, 2)], closed: &["zeta"], real: req("api/auth/login"), shown: req("api/auth/login"), no_fold: false },
            FoldCase { id: "F18", open: &all, at: req("api/pong"), keys: vec![zc, Down], closed: &["api"], real: dir("zeta"), shown: dir("zeta"), no_fold: false },
            FoldCase { id: "F19", open: &all, at: req("api/pong"), keys: vec![zc, Up], closed: &["api"], real: req("top"), shown: req("top"), no_fold: false },
            FoldCase { id: "F20", open: &all, at: dir("api/auth"), keys: vec![zc], closed: &["api/auth"], real: dir("api/auth"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F21", open: &all, at: dir("api/auth"), keys: vec![zc, Down], closed: &["api/auth"], real: dir("zeta"), shown: dir("zeta"), no_fold: false },
            FoldCase { id: "F22", open: &all, at: dir("api"), keys: vec![zc, Down], closed: &["api"], real: dir("zeta"), shown: dir("zeta"), no_fold: false },
            FoldCase { id: "F23", open: &["api", "zeta"], at: req("api/ping"), keys: vec![zc, zo], closed: &["api/auth"], real: req("api/ping"), shown: req("api/ping"), no_fold: false },
            FoldCase { id: "F24", open: &all, at: req("top"), keys: vec![zm], closed: &["api", "api/auth", "zeta"], real: req("top"), shown: req("top"), no_fold: false },
            FoldCase { id: "F25", open: &all, at: req("zeta/z1"), keys: vec![zc, zo], closed: &[], real: req("zeta/z1"), shown: req("zeta/z1"), no_fold: false },
            FoldCase { id: "F26", open: &all, at: req("api/auth/login"), keys: vec![Z(Close, 3)], closed: &["api", "api/auth"], real: req("api/auth/login"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F27", open: &all, at: req("api/auth/login"), keys: vec![zm, zc], closed: &["api", "api/auth", "zeta"], real: req("api/auth/login"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F28", open: &all, at: dir("api"), keys: vec![zm, zc], closed: &["api", "api/auth", "zeta"], real: dir("api"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F29", open: &all, at: req("top"), keys: vec![zr], closed: &[], real: req("top"), shown: req("top"), no_fold: false },
            FoldCase { id: "F30", open: &all, at: req("api/auth/login"), keys: vec![zc, Down], closed: &["api/auth"], real: dir("zeta"), shown: dir("zeta"), no_fold: false },
            FoldCase { id: "F31", open: &all, at: req("api/auth/login"), keys: vec![zc, zc, Up], closed: &["api", "api/auth"], real: req("top"), shown: req("top"), no_fold: false },
            FoldCase { id: "F32", open: &all, at: req("api/pong"), keys: vec![zc, Down, zo], closed: &["api"], real: dir("zeta"), shown: dir("zeta"), no_fold: false },
            FoldCase { id: "F33", open: &all, at: req("api/pong"), keys: vec![zc, zo], closed: &[], real: req("api/pong"), shown: req("api/pong"), no_fold: false },
            FoldCase { id: "F34", open: &all, at: dir("api"), keys: vec![zm, zo], closed: &["api/auth", "zeta"], real: dir("api"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F35", open: &all, at: dir("api/auth"), keys: vec![zm, Z(Open, 2)], closed: &["zeta"], real: dir("api/auth"), shown: dir("api/auth"), no_fold: false },
            FoldCase { id: "F36", open: &all, at: req("api/ping"), keys: vec![zm, zo, zc], closed: &["api", "api/auth", "zeta"], real: req("api/ping"), shown: dir("api"), no_fold: false },
            FoldCase { id: "F37", open: &all, at: req("api/auth/login"), keys: vec![zm, zo, zo], closed: &["zeta"], real: req("api/auth/login"), shown: req("api/auth/login"), no_fold: false },
            FoldCase { id: "F38", open: &all, at: req("api/pong"), keys: vec![zm, zr, zo], closed: &[], real: req("api/pong"), shown: req("api/pong"), no_fold: false },
        ];
        for FoldCase { id, open: open_before, at, keys, closed, real, shown, no_fold } in cases {
            let (mut s, mut open) = probe_tree(open_before, &at);
            let mut noted = false;
            for key in keys {
                noted |= press(&mut s, &mut open, key);
            }
            let shut: Vec<&str> = all.iter().copied().filter(|f| !open.contains(&format!("main/{f}"))).collect();
            assert_eq!(shut, closed, "{id}: the folders shut");
            assert_eq!(s.fold_cursor(), Some(real), "{id}: the cursor's row");
            assert_eq!(s.selected_key(), Some(shown), "{id}: the row it shows on");
            assert_eq!(noted, no_fold, "{id}: No fold found");
        }
    }

    /// Review Focus 5: `zo` acts on the row the cursor is on now. A move, a
    /// click or a folder toggle between `zc` and `zo` forgets the row `zc`
    /// hid, as Vim's cursor leaves it.
    #[test]
    fn a_move_forgets_the_hidden_row() {
        use crate::action::FoldOp::{Close, Open};
        let login = RowKey::Request("main/api/auth/login".into());
        let auth = RowKey::Folder("main/api/auth".into());
        let all = ["api", "api/auth", "zeta"];
        let zc = FoldKey::Z(Close, 1);
        let zo = FoldKey::Z(Open, 1);
        // `k` then `j` comes back to the folder row: the cursor is on it now.
        let (mut s, mut open) = probe_tree(&all, &login);
        press(&mut s, &mut open, zc);
        press(&mut s, &mut open, FoldKey::Up);
        press(&mut s, &mut open, FoldKey::Down);
        assert_eq!(s.fold_cursor(), Some(auth.clone()));
        press(&mut s, &mut open, zo);
        assert_eq!(s.selected_key(), Some(auth.clone()), "zo opened the folder, the cursor stays on it");
        // A click on the folder row is a move too.
        let (mut s, mut open) = probe_tree(&all, &login);
        press(&mut s, &mut open, zc);
        let at = s.selected.expect("on the folder");
        s.select_row(at);
        assert_eq!(s.fold_cursor(), Some(auth.clone()), "a click forgets it");
        // So is a folder toggle (Enter, a click on its arrow).
        let (mut s, mut open) = probe_tree(&all, &login);
        press(&mut s, &mut open, zc);
        assert!(s.toggle_selected_folder().is_some());
        assert_eq!(s.fold_cursor(), Some(auth), "a toggle forgets it");
    }

    /// Sweep backlog item 3, against Vim 9.1 (the cursor at column 0 of a
    /// line inside a closed fold): `h` does not move, so nothing opens and
    /// the cursor keeps its line; `l` moves, and `'foldopen'`'s `hor` opens
    /// every fold around the cursor (probes P1–P7). The folder row showing
    /// the hidden row does the same.
    #[test]
    fn h_and_l_keep_the_hidden_row_as_vims_cursor_does() {
        use crate::action::FoldOp::{Close, Open};
        let login = RowKey::Request("main/api/auth/login".into());
        let api = RowKey::Folder("main/api".into());
        let zc = FoldKey::Z(Close, 1);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let (mut s, mut open) = probe_tree(&["api", "api/auth", "zeta"], &login);
        press(&mut s, &mut open, zc);
        press(&mut s, &mut open, zc);
        assert_eq!(s.selected_key(), Some(api.clone()));
        // P5: `h` changes nothing.
        assert_eq!(s.handle_key(key(KeyCode::Left)), Some(Action::Render));
        assert_eq!(s.selected_key(), Some(api), "h does not leave the folder row");
        assert_eq!(s.fold_cursor(), Some(login.clone()), "h keeps the hidden row");
        // P4: `l` opens both folders around it and lands on it.
        let l = s.handle_key(key(KeyCode::Right));
        assert_eq!(l, Some(Action::SidebarFold { op: Open, count: usize::MAX }));
        press(&mut s, &mut open, FoldKey::Z(Open, usize::MAX));
        assert!(open.contains("main/api") && open.contains("main/api/auth"), "{open:?}");
        assert_eq!(s.selected_key(), Some(login));
    }
}
