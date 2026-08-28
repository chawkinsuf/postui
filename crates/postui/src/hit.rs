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
    /// The right-aligned "theme" chip on the app bar: opens the theme
    /// picker.
    HeaderTheme,
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
    /// The `❐` copy-URL chip at the right edge of the URL well: click copies
    /// the URL text to the clipboard (`Action::CopyToClipboard(CopyTarget::Url)`,
    /// the same path the palette's "Request: copy URL" command uses).
    /// Registered on top of `UrlBar` so it wins the hit test over the well
    /// behind it.
    CopyUrl,
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
    /// empty row that becomes a real option once its key is typed.
    TableCell {
        row: usize,
        col: u8,
    },
    /// The `⌄ hide` / `› show` toggle at the tab strip's right edge.
    TableCollapse,
    /// The Response pane's hide/show toggle at the right edge of its
    /// header strip: click dispatches `Action::ToggleResponseCollapse`.
    ResponseCollapse,
    /// Raw mouse event forwarded to edtui (click-to-place, wheel).
    BodyEditor,
    ResponseTab(crate::components::response::ViewMode),
    CopyBodyButton,
    SaveBodyButton,
    /// The `✎` button on the response header strip: opens the active
    /// tab's text in `$EDITOR` (view-only — nothing is read back).
    ResponseEditorButton,
    /// The `Find` button on the response header strip: opens the in-pane
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
    /// The horizontal scrollbar's thumb (bottom row of a pane whose
    /// content is wider than its viewport — today only the Response body).
    HScrollThumb(PaneId),
    /// Signed page delta in columns applied on click (±viewport width).
    HScrollTrack(PaneId, i16),
    DropdownRow(usize),
    ChooserRow(usize),
    /// The chooser's optional title-row toggle label (the theme picker's
    /// dark/light filter): click dispatches the toggle's action without
    /// closing the modal.
    ChooserToggle,
    PaletteRow(usize),
    VarPickerRow(usize),
    /// A visible row of the Variable Manager's left list (spec §3.4).
    /// Index into `VarManager::left_rows`; a left click opens it in the
    /// detail pane, a right click opens its Rename/Duplicate/Delete menu.
    VmLeftRow(usize),
    /// The Manager top bar's `Environment: <name> ▾` button: opens the same
    /// environment chooser the header's env chip does.
    VmEnvSwitch,
    /// The Manager top bar's `+ Variable` / `+ Group` buttons.
    VmNewVar,
    VmNewSelector,
    /// One field of the variable form's right pane (spec §3.4): click seeds
    /// it with its current text and a caret at the end, exactly like a
    /// table cell (Task 8's in-place model).
    VmFormField(crate::components::varmanager::VmField),
    /// The variable form's `secret [on/off]` toggle: opens the existing
    /// `ToggleSecretVar` confirm.
    VmSecretToggle,
    /// The variable form's `👁 reveal`/`hide` toggle beside a secret's
    /// "Value in <env>" field.
    VmRevealToggle,
    /// The variable form's title-row `[Rename]` button.
    VmRename,
    /// The variable form's title-row `[Delete]` button.
    VmDelete,
    /// The selector grid's `◉`/`○` radio on option row `i` (spec §3.4): a
    /// click selects that option for the active environment, which is what
    /// makes every one of the selector's fields resolve to its values.
    VmEntryRadio(usize),
    /// One cell of the selector grid: `col == 0` is the option-name cell, `col
    /// n` is the selector's `n-1`th field. `row == options.len()` is the ghost
    /// row — the always-present empty row that becomes a real option the
    /// moment its name cell commits non-empty (Task 8's table model).
    VmEntryCell {
        row: usize,
        col: usize,
    },
    /// The selector pane's `[+ Entry]` button.
    VmNewOption,
    /// The selector pane's `[Edit fields]` button: opens the field-list
    /// editor (one text slot per current field plus an empty one).
    VmEditFields,
    /// The variable form's promote/demote button — whichever of the two
    /// applies right now (`VarManager`'s own precondition check decides
    /// which, and which `Action` a click fires).
    VmPromoteBtn,
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
    /// Field `i` of a `Modal::MultiPrompt` (label row + input box): click
    /// moves the prompt's focus there.
    ModalField(usize),
    /// The 3-row text box of field `i` in the top modal's field order
    /// (`Prompt` = 0; `NewProject` name/path = 0/1; `MultiPrompt` = its
    /// free-text fields' indices). Registered over `ModalField`, so the
    /// box itself gets the full text-input mouse treatment — click places
    /// the caret, drag sweeps a selection, double click selects all —
    /// while the label row keeps `ModalField`'s plain focus-click.
    ModalInput(usize),
}

/// A terminal pointer-shape hint (Kitty's OSC 22 protocol, `\x1b]22;{shape}\x07`),
/// computed from the [`Hit`] under the mouse. Terminals that don't support
/// the protocol simply ignore the escape sequence, so this is a no-op
/// enhancement everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerShape {
    /// The terminal's own cursor: background, chrome the mouse can't act on,
    /// and modal-dismiss regions (`ModalOutside`/`ModalBody`, which behave
    /// like background rather than a button).
    Default,
    /// The hand cursor: anything `on_hit` actually dispatches a click
    /// through — buttons, tabs, chips, rows, cells, scrollbars, dropdown
    /// rows.
    Pointer,
    /// The I-beam cursor: surfaces where a click places a text caret or
    /// anchors a text selection. `UrlBar` and `BodyEditor` (free-text
    /// option), plus the response pane's selectable content — its bare
    /// background (`Pane(Response)`, the Raw/Headers views) and the Pretty
    /// view's `JsonRow` lines, whose clicks all anchor a selection sweep.
    /// The in-place table/form cell edits (`TableCell`, `VmFormField`,
    /// `VmEntryCell`) are first a *click* to select/open before any typing
    /// starts and so stay `Pointer`.
    Text,
}

impl PointerShape {
    /// The shape name OSC 22 expects (`pointer`, `default`, `text`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            PointerShape::Default => "default",
            PointerShape::Pointer => "pointer",
            PointerShape::Text => "text",
        }
    }

    /// Maps the currently hovered hit (mirroring `App::hovered`) to the
    /// shape the pointer should show. A blanket "everything but background
    /// chrome is clickable" rule rather than an exhaustive per-variant
    /// match: `Hit` gains new clickable kinds far more often than new
    /// non-interactive ones, and every existing non-interactive kind is
    /// named here explicitly.
    pub fn for_hit(hit: Option<&Hit>) -> Self {
        match hit {
            None => PointerShape::Default,
            Some(Hit::UrlBar | Hit::BodyEditor | Hit::JsonRow(_) | Hit::ModalInput(_)) => {
                PointerShape::Text
            }
            // The response pane's bare content is selectable text (a click
            // anchors a selection sweep), so it I-beams like the editors.
            Some(Hit::Pane(PaneId::Response)) => PointerShape::Text,
            Some(Hit::Pane(_) | Hit::ModalOutside | Hit::ModalBody) => PointerShape::Default,
            Some(_) => PointerShape::Pointer,
        }
    }
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
    h_tracks: Vec<(PaneId, Rect)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
        self.tracks.clear();
        self.h_tracks.clear();
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

    /// Records the full horizontal scrollbar row drawn for `pane` this
    /// frame.
    pub fn register_h_track(&mut self, pane: PaneId, rect: Rect) {
        self.h_tracks.push((pane, rect));
    }

    /// The horizontal scrollbar track drawn for `pane` on the last frame,
    /// if any.
    pub fn h_track_of(&self, pane: PaneId) -> Option<Rect> {
        self.h_tracks
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

    /// Topmost hit containing the point, skipping the modal layer
    /// (`ModalOutside`/`ModalBody`/`DropdownRow`) as well as `VarToken`
    /// overlays: what a click would land on if the open modal weren't
    /// there. Used by right-click re-targeting — a right click while a
    /// context menu is open dismisses it and acts on the control
    /// underneath, whose hits are still registered below the overlay.
    pub fn hit_at_under_modal(&self, x: u16, y: u16) -> Option<&Hit> {
        self.regions
            .iter()
            .rev()
            .filter(|(_, hit)| {
                !matches!(
                    hit,
                    Hit::VarToken(_) | Hit::ModalOutside | Hit::ModalBody | Hit::DropdownRow(_)
                )
            })
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
    fn pointer_shape_maps_background_and_modal_dismiss_regions_to_default() {
        assert_eq!(PointerShape::for_hit(None), PointerShape::Default);
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::Pane(PaneId::Sidebar))),
            PointerShape::Default
        );
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::ModalOutside)),
            PointerShape::Default
        );
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::ModalBody)),
            PointerShape::Default
        );
    }

    #[test]
    fn pointer_shape_maps_text_entry_surfaces_to_text() {
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::UrlBar)),
            PointerShape::Text
        );
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::BodyEditor)),
            PointerShape::Text
        );
        // The response pane's selectable content: bare background
        // (Raw/Headers) and the Pretty view's rows — a click on either
        // anchors a selection sweep, so both I-beam.
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::Pane(PaneId::Response))),
            PointerShape::Text
        );
        assert_eq!(
            PointerShape::for_hit(Some(&Hit::JsonRow(3))),
            PointerShape::Text
        );
    }

    #[test]
    fn pointer_shape_maps_clickable_hits_to_pointer() {
        for hit in [
            Hit::SendButton,
            Hit::MethodSelector,
            Hit::SidebarRow(0),
            Hit::EditorTab(0),
            Hit::TableCell { row: 0, col: 0 },
            Hit::ScrollbarThumb(PaneId::Sidebar),
            Hit::DropdownRow(0),
            Hit::ModalConfirm,
        ] {
            assert_eq!(
                PointerShape::for_hit(Some(&hit)),
                PointerShape::Pointer,
                "{hit:?} should be Pointer"
            );
        }
    }

    #[test]
    fn pointer_shape_as_str_matches_kitty_names() {
        assert_eq!(PointerShape::Default.as_str(), "default");
        assert_eq!(PointerShape::Pointer.as_str(), "pointer");
        assert_eq!(PointerShape::Text.as_str(), "text");
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
