use crate::app::{App, Screen};
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
///
/// `app.screen` (spec §5) branches the body: `Screen::Main` draws the usual
/// three panes; any other screen draws full-frame into `layout.body`
/// instead, while the header and footer stay exactly as they are on
/// `Main`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    // Sampled once and threaded through the whole frame -- matching
    // `DrawCtx::now`'s own documented invariant -- rather than resampling
    // `Instant::now()` at each of this function's several draw calls,
    // which could otherwise see an animation at very slightly different
    // points within the same frame.
    let now = std::time::Instant::now();
    let editor_collapsed_to_chrome = app.table_collapsed
        && matches!(
            app.editor.active_tab,
            crate::components::editor::EditorTab::Params
                | crate::components::editor::EditorTab::Headers
                | crate::components::editor::EditorTab::Vars
        );
    // `App::sync_pane_collapse_anim` (run on every `update`) keeps
    // `AnimKey::PaneCollapse` chasing this same condition, so its eased
    // value at `now` is what actually drives the row split — falling back
    // to the settled bool (as a plain 0.0/1.0) only for the very first
    // frame, before `update` has ever run and started the anim.
    let collapse_t = app.anims.value_or(
        crate::anim::AnimKey::PaneCollapse,
        now,
        if editor_collapsed_to_chrome { 1.0 } else { 0.0 },
    );
    let layout = compute_layout(frame.area(), collapse_t);
    let focus = app.focus;
    let screen = app.screen;
    let mut hits = std::mem::take(&mut app.hits);
    hits.clear();

    let project_name = app.project.display_name();
    let env_label = app.project.env_label();
    crate::components::header_bar::draw_header(
        frame,
        layout.header,
        &app.theme,
        &project_name,
        &env_label,
        screen == Screen::VarManager,
        &mut hits,
        app.hovered.as_ref(),
    );

    match screen {
        Screen::Main => {
            hits.register(layout.sidebar, Hit::Pane(PaneId::Sidebar));
            hits.register(layout.editor, Hit::Pane(PaneId::Editor));
            hits.register(layout.response, Hit::Pane(PaneId::Response));
            // The painted gutter separating the sidebar from the main
            // panes — the surviving separator now that panes no longer
            // draw a `│` border of their own.
            crate::paint::fill(frame.buffer_mut(), layout.gutter, app.theme.page);
            // Recomputed every draw the Headers tab is showing (spec §6,
            // Task 10): cheap (small N), and keeps the computed section
            // live across an env switch or an in-progress edit without
            // having to track exactly which action invalidates it. Gated on
            // the active tab -- there's nothing on screen to read it on any
            // other tab, so skip the two prepare_context-driven passes.
            if app.editor.active_tab == crate::components::editor::EditorTab::Headers {
                let prepare_ctx = app.project.prepare_context();
                app.editor.recompute_computed_headers(&prepare_ctx);
            }
            let hovered = app.hovered.as_ref();
            let dragged_pane = app.drag.as_ref().map(|d| d.pane);
            // Destructured so each component can be borrowed mutably
            // alongside the shared theme reference its DrawCtx holds.
            let App {
                theme,
                sidebar,
                editor,
                session,
                anims,
                ..
            } = app;
            let response = &mut session.response;
            let ctx = |pane: PaneId| DrawCtx {
                theme,
                focused: focus == pane,
                hovered,
                dragging: dragged_pane == Some(pane),
                anims,
                now,
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
        }
        Screen::VarManager => {
            let open_request = app
                .editor
                .slug
                .is_some()
                .then(|| app.editor.current_request());
            app.varmanager.draw(
                frame,
                layout.body,
                &app.theme,
                &app.project,
                open_request.as_ref(),
                &mut hits,
                app.hovered.as_ref(),
            );
        }
        Screen::Testbed => {
            let ctx = DrawCtx {
                theme: &app.theme,
                focused: false,
                hovered: app.hovered.as_ref(),
                dragging: false,
                anims: &app.anims,
                now,
            };
            crate::components::testbed::draw_testbed(frame, layout.body, &ctx);
        }
    }

    crate::components::footer::draw_footer(
        frame,
        layout.footer,
        &app.theme,
        focus,
        app.shift_enter_send,
        app.editor.sending,
        &mut hits,
        app.hovered.as_ref(),
    );
    app.toasts
        .draw(frame, frame.area(), &app.theme, &app.anims, now);
    app.modals.draw(
        frame,
        frame.area(),
        &app.theme,
        &mut hits,
        app.hovered.as_ref(),
        &app.keymap,
        &app.anims,
        now,
    );
    app.hits = hits;

    // The variable tooltip is painted last of all, over every pane — after
    // the hit map is back on `app`, because a caret-raised tip is anchored
    // at the `VarToken` rect this very frame registered. It never covers a
    // dialog: `var_token_tip` yields nothing while a modal is up.
    if let Some(tip) = app.var_token_tip() {
        draw_var_tooltip(frame, frame.area(), &app.theme, &tip, &app.editor.vars);
    }
}

/// Height of the variable tooltip: a padding row, the `name = value` line,
/// the source line, and a closing padding row.
const TOOLTIP_HEIGHT: u16 = 4;

/// Draws the hover/caret tooltip for one `{{token}}` (spec §7): line 1 is
/// `name = value` — always `SECRET_MASK` for a secret, with no reveal
/// anywhere — and line 2 names the scope the value came from (`request var`,
/// `env qa`, `default`, `group user → "user 2"`, `needs selection`, `missing
/// secret`). It sits under the token it belongs to, flipping above when
/// there is no room below, and is clamped to stay inside `screen`.
fn draw_var_tooltip(
    frame: &mut Frame,
    screen: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
    tip: &crate::app::TokenTip,
    vars: &crate::components::var_tokens::VarView,
) {
    use ratatui::layout::Rect;
    let info = vars.describe(&tip.name);
    // A long value would otherwise stretch the tooltip past the terminal;
    // the full value is always available in the Variable Manager.
    let line1 = ellipsize(&format!("{} = {}", tip.name, info.display_value()), 56);
    let line2 = info.source.label();
    let text_w = line1.chars().count().max(line2.chars().count()) as u16;
    // 2 columns of padding each side, plus a column for the drop shadow.
    let width = (text_w + 4).min(screen.width.saturating_sub(1));
    if width < 5 || screen.height < TOOLTIP_HEIGHT {
        return;
    }
    let below = tip.anchor.bottom();
    let y = if below + TOOLTIP_HEIGHT <= screen.bottom() {
        below
    } else {
        tip.anchor.y.saturating_sub(TOOLTIP_HEIGHT)
    };
    let x = tip
        .anchor
        .x
        .min(screen.right().saturating_sub(width + 1))
        .max(screen.x);
    let area = Rect::new(x, y, width, TOOLTIP_HEIGHT);
    let buf = frame.buffer_mut();
    crate::paint::floating_panel(buf, area, screen, theme);
    let inner = width.saturating_sub(4) as usize;
    crate::paint::text(
        buf,
        x + 2,
        y + 1,
        &ellipsize(&line1, inner),
        theme.text,
        theme.panel,
        true,
    );
    crate::paint::text(
        buf,
        x + 2,
        y + 2,
        &ellipsize(&line2, inner),
        theme.text_muted,
        theme.panel,
        false,
    );
}

/// `s` cut to at most `max` characters, the last of which becomes `…`.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// Marks the focused pane with a half-block accent bar down its left
/// padding column, so Tab's current target is always visible at a glance.
/// Painted after the pane's own draw, and glyph/fg only — each cell keeps
/// the background under it (the sidebar's row highlights run to column 0).
/// The sidebar's selected-row marker (`ListRow`'s accent bar) lives in this
/// same column and is the stronger mark, so the focus bar leaves it alone —
/// both use the same `▌` glyph, so a selected row already reads as the
/// focus bar continuing through it.
fn focus_bar(
    buf: &mut ratatui::buffer::Buffer,
    pane: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
) {
    for y in pane.y..pane.y + pane.height {
        if let Some(cell) = buf.cell_mut((pane.x, y)) {
            if cell.symbol() == "▌" && cell.fg == theme.accent {
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
        assert!(!content.contains("REQUESTS")); // no sidebar header; the button is the identity
        assert!(content.contains("New request")); // sidebar's + New request button
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
        let layout = crate::layout::compute_layout(ratatui::layout::Rect::new(0, 0, 120, 40), 0.0);
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
