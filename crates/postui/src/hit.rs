use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// A typed clickable target. Registered with its screen Rect during render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// Pane background: click focuses, wheel scrolls.
    Pane(PaneId),
    HeaderProject,
    HeaderEnv,
    /// The header's "vars" chip: toggles the Variable Manager screen.
    HeaderVars,
    /// A footer hint chip that dispatches its action on click.
    FooterChip(crate::action::Action),
    SidebarNewRequest,
    /// Index into `sidebar.rows`.
    SidebarRow(usize),
    SidebarFolderArrow(usize),
    /// Renders as Cancel while a request is in flight.
    SendButton,
    MethodSelector,
    /// The address bar's URL segment: click focuses the URL line and places
    /// the caret at the clicked column.
    UrlBar,
    /// 0 = Params, 1 = Headers, 2 = Body.
    EditorTab(usize),
    /// A table row's background: hover and right-click target. The row's
    /// own cells are registered on top of it, so an ordinary left click
    /// lands in a cell (`TableCell`) rather than here.
    TableRow(usize),
    TableCheckbox(usize),
    /// The `✕` delete affordance on the active (expanded) row.
    TableDelete(usize),
    /// One editable cell of a key/value table: `col` 0 is the key, 1 the
    /// value. `row == map.len()` is the ghost row — the always-present
    /// empty row that becomes a real entry once its key is typed.
    TableCell {
        row: usize,
        col: u8,
    },
    /// The `⌄ hide` / `› show` toggle at the tab strip's right edge.
    TableCollapse,
    /// Raw mouse event forwarded to edtui (click-to-place, wheel).
    BodyEditor,
    ResponseTab(crate::components::response::ViewMode),
    CopyBodyButton,
    SaveBodyButton,
    /// The `⌕` button on the response header strip: opens the in-pane
    /// search, exactly as `/` does.
    ResponseSearchButton,
    /// The `▼`/`▲` buttons beside the search footer: step to the next /
    /// previous match, exactly as `n`/`N` do. Registered only while a
    /// search is open.
    ResponseSearchNext,
    ResponseSearchPrev,
    /// Copy icon on row `i` of the response Headers view.
    HeaderCopy(usize),
    /// Copy icon on row `i` (index into the computed-headers section's own
    /// display order — the request table's rows are excluded and have no
    /// hit of this kind) of the request Headers tab's computed section.
    AutoHeaderCopy(usize),
    /// The single "reveal"/"hide" toggle shown above the computed-headers
    /// section when at least one row would otherwise show a masked secret.
    AutoHeaderReveal,
    /// Visible row `i` of the JSON tree (click selects).
    JsonRow(usize),
    /// The ▸/▾ glyph cell of visible row `i` (click toggles).
    JsonArrow(usize),
    ScrollbarThumb(PaneId),
    /// Signed page delta applied on click (±viewport height).
    ScrollbarTrack(PaneId, i16),
    DropdownRow(usize),
    ChooserRow(usize),
    PaletteRow(usize),
    VarPickerRow(usize),
    /// A visible row of the Variable Manager grid (spec §5), background
    /// click target. Index into `VarManager::rows`.
    VarRow(usize),
    /// The name region of a Variable Manager row (the first `NAME_W`
    /// cells): double-click renames a variable (or expands an expandable
    /// row), unlike the description region next to it (`VarCell` col 0).
    VarName(usize),
    /// One visible cell of the Variable Manager grid: `row` indexes
    /// `VarManager::rows`; `col` 0 is the shared name/desc block, `col`
    /// 1.. are environment columns (relative to `env_scroll`).
    VarCell {
        row: usize,
        col: usize,
    },
    /// One drawn `{{name}}` token, carrying the variable's name (spec §7).
    /// Registered *over* whatever control the token sits on (URL bar, table
    /// cell, computed-header row, body editor), so a left click opens the
    /// var picker prefiltered to that name. Deliberately invisible to
    /// hover styling and to right-click menus — see
    /// [`HitMap::hit_at_ignoring_var_tokens`].
    VarToken(String),
    /// A clickable `[y] Label` chip in a Confirm modal.
    ConfirmChoice(char),
    /// The top modal's painted Cancel button (Message has none; Prompt and
    /// NewProject each have one). Click parity with `Esc`: the app-side
    /// handler synthesizes an `Esc` key event into `ModalStack::handle_key`
    /// so it goes through the exact same per-variant logic Esc already
    /// does, whichever modal is on top.
    ModalCancel,
    /// The top modal's painted primary confirm button (Message's "OK",
    /// Prompt's and NewProject's "Confirm"). Click parity with `Enter`:
    /// same synthesize-the-key-event approach as `ModalCancel`.
    ModalConfirm,
    /// Full-screen region under an open modal; click closes (same as Esc).
    ModalOutside,
    /// A modal's own box (borders/body), registered over `ModalOutside` so
    /// clicking the modal's chrome — anywhere that isn't one of its own
    /// interactive hits — does nothing instead of closing it.
    ModalBody,
}

/// Rebuilt each frame during render; maps screen regions to typed [`Hit`]s.
///
/// It also carries each frame's scrollbar *track* rects. Those are not
/// clickable regions of their own (the track's two page segments and the
/// thumb are), but drag handling needs the whole column's geometry to turn a
/// pointer row into a thumb top, and the track is only known at draw time.
/// Parking it here rather than in a second per-frame structure keeps every
/// component's `draw` signature (and `ui::draw`'s threading) unchanged, and
/// means a single `clear()` resets all of a frame's artifacts at once.
#[derive(Default)]
pub struct HitMap {
    regions: Vec<(Rect, Hit)>,
    tracks: Vec<(PaneId, Rect)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
        self.tracks.clear();
    }

    /// Records the full scrollbar column drawn for `pane` this frame.
    pub fn register_track(&mut self, pane: PaneId, rect: Rect) {
        self.tracks.push((pane, rect));
    }

    /// The scrollbar track drawn for `pane` on the last frame, if any.
    pub fn track_of(&self, pane: PaneId) -> Option<Rect> {
        self.tracks
            .iter()
            .rev()
            .find(|(p, _)| *p == pane)
            .map(|(_, r)| *r)
    }

    pub fn register(&mut self, rect: Rect, hit: Hit) {
        self.regions.push((rect, hit));
    }

    /// Topmost (= last registered) hit containing the point.
    pub fn hit_at(&self, x: u16, y: u16) -> Option<&Hit> {
        self.regions
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(ratatui::layout::Position { x, y }))
            .map(|(_, hit)| hit)
    }

    /// Topmost hit containing the point, skipping [`Hit::VarToken`]
    /// overlays: a token sits *on* a control, and hover styling and
    /// right-click menus belong to the control under it (a hovered row must
    /// not lose its highlight because the pointer crossed a `{{token}}` in
    /// its value). Token hovering is tracked separately, by
    /// [`HitMap::var_token_at`].
    pub fn hit_at_ignoring_var_tokens(&self, x: u16, y: u16) -> Option<&Hit> {
        self.regions
            .iter()
            .rev()
            .filter(|(_, hit)| !matches!(hit, Hit::VarToken(_)))
            .find(|(rect, _)| rect.contains(ratatui::layout::Position { x, y }))
            .map(|(_, hit)| hit)
    }

    /// The topmost drawn `{{token}}` under the point: its name and the rect
    /// it was drawn into (the tooltip's anchor).
    pub fn var_token_at(&self, x: u16, y: u16) -> Option<(&str, Rect)> {
        self.regions.iter().rev().find_map(|(rect, hit)| match hit {
            Hit::VarToken(name) if rect.contains(ratatui::layout::Position { x, y }) => {
                Some((name.as_str(), *rect))
            }
            _ => None,
        })
    }

    /// Topmost `Hit::Pane` containing the point (for wheel routing).
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.regions.iter().rev().find_map(|(rect, hit)| match hit {
            Hit::Pane(pane) if rect.contains(ratatui::layout::Position { x, y }) => Some(*pane),
            _ => None,
        })
    }

    /// Last-registered rect for `hit` — the test helper click tests use.
    pub fn rect_of(&self, hit: &Hit) -> Option<Rect> {
        self.regions
            .iter()
            .rev()
            .find(|(_, h)| h == hit)
            .map(|(rect, _)| *rect)
    }
}

/// Everything a vertical scrollbar needs: where the viewport sits in the
/// content (`offset`), how much content there is, and how much of it fits.
/// A pane reserves a column for the bar exactly when `content > viewport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarSpec {
    pub pane: PaneId,
    pub offset: usize,
    pub content: usize,
    pub viewport: usize,
}

impl ScrollbarSpec {
    /// True when the content overflows and a bar is warranted.
    pub fn overflows(&self) -> bool {
        self.content > self.viewport && self.viewport > 0
    }

    /// The largest legal `offset`: scrolled to the very bottom.
    pub fn max_offset(&self) -> usize {
        self.content.saturating_sub(self.viewport)
    }
}

/// Integer `a * b / c`, rounded to nearest (ties up). `c` must be non-zero.
fn scale(a: usize, b: usize, c: usize) -> usize {
    (a * b + c / 2) / c
}

/// `(thumb_top_row_within_track, thumb_height)` — the thumb is proportional
/// to the visible fraction, at least one row tall, and reaches the bottom of
/// the track exactly at [`ScrollbarSpec::max_offset`].
pub fn thumb_geometry(spec: &ScrollbarSpec, track_h: u16) -> (u16, u16) {
    if track_h == 0 {
        return (0, 0);
    }
    let track = track_h as usize;
    let height = scale(track, spec.viewport, spec.content.max(1)).clamp(1, track);
    let max_top = track - height;
    let max_offset = spec.max_offset();
    let top = if max_top == 0 || max_offset == 0 {
        0
    } else {
        scale(spec.offset.min(max_offset), max_top, max_offset)
    };
    (top as u16, height as u16)
}

/// Inverse of [`thumb_geometry`]'s top: the content offset that would draw
/// the thumb at `thumb_top`. Clamped to the pane's legal offset range.
pub fn offset_for_thumb_top(spec: &ScrollbarSpec, track_h: u16, thumb_top: u16) -> usize {
    let (_, height) = thumb_geometry(spec, track_h);
    let max_top = track_h.saturating_sub(height) as usize;
    let max_offset = spec.max_offset();
    if max_top == 0 {
        return 0;
    }
    scale((thumb_top as usize).min(max_top), max_offset, max_top).min(max_offset)
}

/// Renders a 1-cell-wide vertical scrollbar into `column` when the content
/// overflows: a dim `│` track under an accent `█` thumb (brightened to
/// text-on-accent while hovered or dragged). Registers
/// `ScrollbarTrack(pane, -viewport)` above the thumb and
/// `ScrollbarTrack(pane, +viewport)` below it, then the thumb on top, and
/// records the track rect for drag geometry. A no-op when the content fits.
pub fn draw_scrollbar(
    frame: &mut Frame,
    hits: &mut HitMap,
    column: Rect,
    spec: &ScrollbarSpec,
    hovered: Option<&Hit>,
    dragging: bool,
    theme: &Theme,
) {
    if !spec.overflows() || column.width == 0 || column.height == 0 {
        return;
    }
    let track = Rect { width: 1, ..column };
    let (top, height) = thumb_geometry(spec, track.height);

    let track_style = Style::default().fg(theme.text_muted);
    frame.render_widget(
        Paragraph::new(
            (0..track.height)
                .map(|_| Line::styled("\u{2502}", track_style))
                .collect::<Vec<_>>(),
        ),
        track,
    );

    let thumb_hit = Hit::ScrollbarThumb(spec.pane);
    // A full block hides its own background, so the usual hover inversion
    // (surface fg on accent bg) would paint the thumb in the *background*
    // color and read as the thumb vanishing. The active thumb brightens to
    // the foreground text color over accent instead.
    let thumb_style = if dragging || hovered == Some(&thumb_hit) {
        Style::default().bg(theme.accent).fg(theme.text)
    } else {
        Style::default().fg(theme.accent)
    };
    let thumb = Rect {
        y: track.y + top,
        height,
        ..track
    };
    frame.render_widget(
        Paragraph::new(
            (0..height)
                .map(|_| Line::styled("\u{2588}", thumb_style))
                .collect::<Vec<_>>(),
        ),
        thumb,
    );

    // Page segments first so the thumb wins where they would overlap.
    let page = spec.viewport.min(i16::MAX as usize) as i16;
    if top > 0 {
        hits.register(
            Rect {
                height: top,
                ..track
            },
            Hit::ScrollbarTrack(spec.pane, -page),
        );
    }
    let below_y = thumb.y + height;
    if below_y < track.y + track.height {
        hits.register(
            Rect {
                y: below_y,
                height: track.y + track.height - below_y,
                ..track
            },
            Hit::ScrollbarTrack(spec.pane, page),
        );
    }
    hits.register(thumb, thumb_hit);
    hits.register_track(spec.pane, track);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn last_registered_hit_wins_at_a_point() {
        let mut m = HitMap::default();
        m.register(Rect::new(0, 0, 10, 10), Hit::Pane(PaneId::Sidebar));
        m.register(Rect::new(2, 2, 3, 1), Hit::SidebarRow(0));
        assert_eq!(m.hit_at(3, 2), Some(&Hit::SidebarRow(0)));
        assert_eq!(m.hit_at(0, 0), Some(&Hit::Pane(PaneId::Sidebar)));
        assert_eq!(m.hit_at(50, 50), None);
        assert_eq!(
            m.pane_at(3, 2),
            Some(PaneId::Sidebar),
            "pane_at sees through overlays"
        );
        assert_eq!(m.rect_of(&Hit::SidebarRow(0)), Some(Rect::new(2, 2, 3, 1)));
    }

    fn spec(offset: usize) -> ScrollbarSpec {
        ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset,
            content: 100,
            viewport: 10,
        }
    }

    #[test]
    fn thumb_geometry_is_proportional_and_round_trips() {
        // 10% of the content is visible -> a 1-row thumb in a 10-row track.
        assert_eq!(thumb_geometry(&spec(0), 10), (0, 1));
        assert_eq!(
            thumb_geometry(&spec(90), 10),
            (9, 1),
            "max offset parks the thumb on the last track row"
        );
        assert_eq!(
            thumb_geometry(&spec(45), 10).0,
            5,
            "halfway through the content is halfway down the track"
        );

        // Thumb height tracks the visible fraction, and never vanishes.
        let half = ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: 0,
            content: 40,
            viewport: 20,
        };
        assert_eq!(thumb_geometry(&half, 10).1, 5);
        let tiny = ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: 0,
            content: 100_000,
            viewport: 10,
        };
        assert_eq!(thumb_geometry(&tiny, 10).1, 1, "min thumb height is 1");

        // Every reachable thumb top maps to an offset that redraws it there.
        for top in 0..=9u16 {
            let offset = offset_for_thumb_top(&spec(0), 10, top);
            assert_eq!(
                thumb_geometry(&spec(offset), 10).0,
                top,
                "round trip for thumb top {top}"
            );
        }
        assert_eq!(
            offset_for_thumb_top(&spec(0), 10, 9),
            90,
            "the bottom of the track is the max offset"
        );
        assert_eq!(
            offset_for_thumb_top(&spec(0), 10, 200),
            90,
            "a thumb top past the track clamps to the max offset"
        );
    }

    #[test]
    fn hovered_thumb_brightens_rather_than_inverting_away() {
        let theme = Theme::for_terminal();
        let area = Rect::new(0, 0, 1, 10);
        let draw = |hovered: Option<&Hit>| {
            let backend = TestBackend::new(1, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            let mut hits = HitMap::default();
            terminal
                .draw(|f| draw_scrollbar(f, &mut hits, area, &spec(0), hovered, false, &theme))
                .unwrap();
            terminal.backend().buffer()[(0, 0)].clone()
        };

        let rest = draw(None);
        assert_eq!(rest.symbol(), "\u{2588}");
        assert_eq!(rest.fg, theme.accent);

        // A full block hides its background, so the active thumb must change
        // its *foreground* to stay visible.
        let active = draw(Some(&Hit::ScrollbarThumb(PaneId::Sidebar)));
        assert_eq!(active.bg, theme.accent);
        assert_eq!(active.fg, theme.text);
        assert_ne!(active.fg, theme.page, "must not vanish into the pane");
    }

    #[test]
    fn scrollbar_is_skipped_when_content_fits() {
        let fits = ScrollbarSpec {
            pane: PaneId::Sidebar,
            offset: 0,
            content: 4,
            viewport: 10,
        };
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(1, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        terminal
            .draw(|f| {
                draw_scrollbar(
                    f,
                    &mut hits,
                    Rect::new(0, 0, 1, 10),
                    &fits,
                    None,
                    false,
                    &theme,
                )
            })
            .unwrap();
        assert_eq!(hits.rect_of(&Hit::ScrollbarThumb(PaneId::Sidebar)), None);
        assert_eq!(hits.track_of(PaneId::Sidebar), None);
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(!content.contains('\u{2588}'), "no thumb glyph when it fits");
    }
}
