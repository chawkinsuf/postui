use crate::app::App;
use crate::components::{Component, DrawCtx};
use crate::hit::Hit;
use crate::layout::{PaneId, compute_layout};
use ratatui::Frame;

/// Takes `&mut App` because components draw through `Component::draw(&mut
/// self, ..)`: the body editor's widget needs `&mut EditorState` to record the
/// viewport it was rendered into.
///
/// The `HitMap` is rebuilt every frame: taken out of `app` up front (so it
/// can be threaded through each draw call as an independent `&mut` borrow
/// alongside `app`'s other fields), cleared, and put back at the end. Pane
/// rects are registered first so any hit a component registers later (a
/// button, a row) is topmost at that point per [`HitMap::hit_at`]'s
/// last-registered-wins rule.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let editor_collapsed_to_chrome = app.table_collapsed
        && matches!(
            app.editor.active_tab,
            crate::components::editor::EditorTab::Params
                | crate::components::editor::EditorTab::Headers
        );
    let layout = compute_layout(frame.area(), editor_collapsed_to_chrome);
    let focus = app.focus;
    let mut hits = std::mem::take(&mut app.hits);
    hits.clear();
    hits.register(layout.sidebar, Hit::Pane(PaneId::Sidebar));
    hits.register(layout.editor, Hit::Pane(PaneId::Editor));
    hits.register(layout.response, Hit::Pane(PaneId::Response));
    // The painted gutter separating the sidebar from the main panes — the
    // surviving separator now that panes no longer draw a `│` border of
    // their own.
    crate::paint::fill(frame.buffer_mut(), layout.gutter, app.theme.page);
    let hovered = app.hovered.as_ref();
    let dragged_pane = app.drag.as_ref().map(|d| d.pane);
    // Destructured so each component can be borrowed mutably alongside the
    // shared theme reference its DrawCtx holds.
    let App {
        theme,
        sidebar,
        editor,
        session,
        toasts,
        modals,
        ..
    } = app;
    let response = &mut session.response;
    crate::components::header_bar::draw_header(
        frame,
        layout.header,
        theme,
        &app.project.display_name(),
        &app.project.env_label(),
        &mut hits,
        hovered,
    );
    let ctx = |pane: PaneId| DrawCtx {
        theme,
        focused: focus == pane,
        hovered,
        dragging: dragged_pane == Some(pane),
    };
    sidebar.draw(frame, layout.sidebar, &ctx(PaneId::Sidebar), &mut hits);
    editor.draw(frame, layout.editor, &ctx(PaneId::Editor), &mut hits);
    response.draw(frame, layout.response, &ctx(PaneId::Response), &mut hits);
    let focused_rect = match focus {
        PaneId::Sidebar => layout.sidebar,
        PaneId::Editor => layout.editor,
        PaneId::Response => layout.response,
    };
    focus_bar(frame.buffer_mut(), focused_rect, theme);
    crate::components::footer::draw_footer(frame, layout.footer, theme, focus, &mut hits, hovered);
    toasts.draw(frame, frame.area(), theme);
    modals.draw(frame, frame.area(), theme, &mut hits, hovered);
    app.hits = hits;
}

/// Marks the focused pane with a half-block accent bar down its left
/// padding column, so Tab's current target is always visible at a glance.
/// Painted after the pane's own draw, and glyph/fg only — each cell keeps
/// the background under it (the sidebar's row highlights run to column 0).
/// The sidebar's selected-row marker (`PillRow`'s accent pill) lives in
/// this same column and is the stronger mark, so the bar keeps the pill's
/// full-block `█` text row. Only that row: the pill's `▄`/`▀` cap glyphs
/// are half-height and cannot coexist with a vertical bar in one cell —
/// letting them through reads as a gap in the bar, so they are flattened
/// into it and the marker shows as a single full-block cell.
fn focus_bar(
    buf: &mut ratatui::buffer::Buffer,
    pane: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
) {
    for y in pane.y..pane.y + pane.height {
        if let Some(cell) = buf.cell_mut((pane.x, y)) {
            if cell.symbol() == "█" && cell.fg == theme.accent {
                continue;
            }
            cell.set_symbol("▌");
            cell.set_fg(theme.focus_ring);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn full_frame_shows_all_panes_and_chrome() {
        let mut app = App::new_for_test();
        let content = render(&mut app);
        assert!(content.contains("REQUESTS")); // sidebar title
        assert!(content.contains("postui")); // header bar app name
        assert!(content.contains("no env")); // header env selector placeholder
        assert!(content.contains("quit")); // footer hint mentions quit key
        assert!(content.contains("No requests yet")); // sidebar empty state
        assert!(content.contains("response will appear here")); // response empty state
        assert!(content.contains("GET")); // editor method badge (default method)
        assert!(content.contains("Params")); // editor tab bar
        assert!(content.contains("Headers")); // editor tab bar
        assert!(content.contains("Body")); // editor tab bar
    }

    /// Panes carry no border or title of their own anymore: no `│` pane
    /// separator, no rounded-corner glyphs anywhere, and the 1-col gutter
    /// between the sidebar and the main panes is a flat `theme.page` fill.
    #[test]
    fn no_pane_borders_and_the_gutter_is_a_flat_page_fill() {
        let mut app = App::new_for_test();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let layout =
            crate::layout::compute_layout(ratatui::layout::Rect::new(0, 0, 120, 40), false);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();

        for glyph in ['╭', '╮', '╰', '╯'] {
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    assert_ne!(
                        buf[(x, y)].symbol(),
                        glyph.to_string(),
                        "no pane border corner at ({x},{y})"
                    );
                }
            }
        }

        assert_eq!(layout.gutter.width, 1);
        for y in layout.gutter.y..layout.gutter.y + layout.gutter.height {
            let cell = buf[(layout.gutter.x, y)].clone();
            assert_eq!(
                cell.symbol(),
                " ",
                "no `│` glyph at the gutter column: {cell:?}"
            );
            assert_eq!(
                cell.bg, app.theme.page,
                "gutter cell is a flat page fill: {cell:?}"
            );
        }
    }
}
