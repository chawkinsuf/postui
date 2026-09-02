use crate::app::{App, Screen};
use crate::components::{Component, DrawCtx};
use crate::hit::Hit;
use crate::layout::{PaneId, compute_layout};
use ratatui::Frame;
use ratatui::layout::Rect;

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
    // Hide collapses the editor to its strip on every tab — the Body tab's
    // buffer hides just like the Params/Headers/Vars table does.
    // `App::sync_pane_collapse_anim` (run on every `update`) keeps
    // `AnimKey::PaneCollapse` chasing this same condition, so its eased
    // value at `now` is what actually drives the row split — falling back
    // to the settled bool (as a plain 0.0/1.0) only for the very first
    // frame, before `update` has ever run and started the anim.
    let collapse_t = app.anims.value_or(
        crate::anim::AnimKey::PaneCollapse,
        now,
        if app.table_collapsed { 1.0 } else { 0.0 },
    );
    let response_t = app.anims.value_or(
        crate::anim::AnimKey::ResponseCollapse,
        now,
        if app.session.response.collapsed {
            1.0
        } else {
            0.0
        },
    );
    let ratio_t = app.anims.value_or(
        crate::anim::AnimKey::SplitRatio,
        now,
        app.split_ratio.editor_share(),
    );
    let layout = compute_layout(frame.area(), collapse_t, response_t, ratio_t);
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
        &app.project.active_space,
        &env_label,
        screen == Screen::Manage,
        // The save/discard group shows only where its keys actually work:
        // the Main screen (`ctrl+s` is not on other screens' whitelist)
        // with no modal capturing the keyboard — and only while there is
        // something to save. An in-progress cell edit counts: it isn't in
        // `is_dirty`'s diff until it commits, but a mouse-only save must
        // be clickable while it's being typed (save commits it first).
        (app.editor.is_dirty() || app.editor.table.editing.is_some())
            && screen == Screen::Main
            && app.modals.top().is_none(),
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
        Screen::Manage => {
            let bar = Rect {
                height: crate::components::manage::BAR_HEIGHT.min(layout.body.height),
                ..layout.body
            };
            let body = Rect {
                y: layout.body.y + bar.height,
                height: layout.body.height - bar.height,
                ..layout.body
            };
            // The strip's underline follows the app's eased edges while a
            // tab switch glides; untracked (the screen just opened) it
            // sits on the active tab's static span.
            let underline = {
                use crate::anim::{AnimKey, StripId};
                let left = app
                    .anims
                    .value(AnimKey::TabUnderline(StripId::ManageTabs), now);
                let right = app
                    .anims
                    .value(AnimKey::TabUnderlineWidth(StripId::ManageTabs), now);
                left.zip(right).map(|(l, r)| (l, r - l))
            };
            crate::components::manage::draw_manage_bar(
                frame,
                bar,
                &app.theme,
                app.manage.tab,
                underline,
                &mut hits,
                app.hovered.as_ref(),
            );
            match app.manage.tab {
                crate::components::manage::ManageTab::Variables => {
                    let open_request = app
                        .editor
                        .slug
                        .is_some()
                        .then(|| app.editor.current_request());
                    app.varmanager.draw(
                        frame,
                        body,
                        &app.theme,
                        &app.project,
                        open_request.as_ref(),
                        &mut hits,
                        app.hovered.as_ref(),
                    );
                }
                tab => {
                    let requests = app.sidebar.space_requests();
                    app.manage.list.draw(
                        frame,
                        body,
                        &app.theme,
                        tab,
                        &app.project,
                        &requests,
                        &mut hits,
                        app.hovered.as_ref(),
                    );
                }
            }
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

    // The Manage screen swaps in its own chip set — the per-pane
    // chips' actions target requests, which aren't on screen there. An
    // open modal with chips of its own wins over both: while it captures
    // the keyboard, its quick actions are the only ones that work.
    let modal_chips = app.modals.footer_chips();
    let globals_live = modal_chips.is_none();
    // A plain `q` reaches the quit binding only where nothing consumes it
    // first: no modal (they capture all input), the Main screen (other
    // screens swallow unbound plain keys), and a focus stop that doesn't
    // route typing into a text input — in the editor pane that is only
    // the URL line, the body, and a live cell edit.
    let plain_q_quits = app.modals.is_empty()
        && (app.screen == Screen::Main
            && (matches!(focus, PaneId::Sidebar | PaneId::Response)
                || focus == PaneId::Editor && !app.editor.plain_keys_type())
            // The manager binds plain q to quit in every focus stop; only
            // a live edit types it.
            || app.screen == Screen::Manage
                && (app.manage.tab != crate::components::manage::ManageTab::Variables
                    || app.varmanager.form.editing.is_none()
                        && app.varmanager.grid.editing.is_none()));
    let vm_chips = modal_chips.or_else(|| {
        (app.screen == Screen::Manage).then(|| {
            if app.manage.tab != crate::components::manage::ManageTab::Variables {
                return app
                    .manage
                    .list
                    .footer_chips(app.manage.tab, &app.project)
                    .into_iter()
                    .map(|(k, l, a)| (k.to_string(), l.to_string(), a))
                    .collect();
            }
            let open_request = app
                .editor
                .slug
                .is_some()
                .then(|| app.editor.current_request());
            app.varmanager
                .footer_chips(&app.project, open_request.as_ref())
                .into_iter()
                .map(|(k, l, a)| (k.to_string(), l.to_string(), a))
                .collect()
        })
    });
    // A selected data row (content focus, no cell edit — space/d would
    // type into one) advertises its toggle/delete keys.
    let table_row_selected = (focus == PaneId::Editor
        && app.editor.sub_focus == crate::components::editor::SubFocus::Content
        && app.editor.table.editing.is_none())
    .then(|| {
        app.editor
            .table
            .selected
            .filter(|s| *s < app.editor.table_len())
            .map(|i| (i, app.editor.table_row_enabled(i)))
    })
    .flatten();
    crate::components::footer::draw_footer(
        frame,
        layout.footer,
        &app.theme,
        focus,
        app.shift_enter_send,
        app.editor.sending,
        // No add chip while a new row is already mid-add (the ghost-row
        // edit); editing an existing row keeps it.
        (!app.editor.adding_row())
            .then(|| app.editor.active_tab.add_row_label())
            .flatten(),
        matches!(
            app.editor.sub_focus,
            crate::components::editor::SubFocus::Method | crate::components::editor::SubFocus::Url
        ),
        table_row_selected,
        vm_chips,
        globals_live,
        plain_q_quits,
        &mut hits,
        app.hovered.as_ref(),
    );
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
    // Toasts paint over the modal stack: a validation error raised by a
    // modal's own confirm must arrive at full strength, not dimmed under
    // the backdrop it is commenting on. They stack bottom-right, and the
    // footer is carved out of their rect so its chips stay readable.
    let toast_area = ratatui::layout::Rect {
        height: frame
            .area()
            .height
            .saturating_sub(crate::components::footer::FOOTER_HEIGHT),
        ..frame.area()
    };
    app.toasts
        .draw(frame, toast_area, &app.theme, &app.anims, now);
    app.hits = hits;

    // The variable tooltip is painted last of all, over every pane — after
    // the hit map is back on `app`, because a caret-raised tip is anchored
    // at the `VarToken` rect this very frame registered. It never covers a
    // dialog: `var_token_tip` yields nothing while a modal is up.
    if let Some(tip) = app.var_token_tip() {
        draw_var_tooltip(frame, frame.area(), &app.theme, &tip, &app.editor.vars);
    }
}

/// Widest a tooltip line may run before the value wraps onto another row.
const TOOLTIP_MAX_TEXT_W: usize = 56;

/// Draws the hover/caret tooltip for one `{{token}}` (spec §7): first the
/// value — always `SECRET_MASK` for a secret, with no reveal anywhere,
/// wrapped onto further rows rather than truncated so the whole value is
/// readable — then a line naming the scope the value came from (`this
/// request`, `env = qa`, `default`, `option = user 2`, `needs selection`,
/// `missing secret`). It sits under the token it belongs to,
/// flipping above when there is no room below, and is clamped to stay
/// inside `screen`.
fn draw_var_tooltip(
    frame: &mut Frame,
    screen: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
    tip: &crate::app::TokenTip,
    vars: &crate::components::var_tokens::VarView,
) {
    use ratatui::layout::Rect;
    let info = vars.describe(&tip.name);
    let mut value_lines = wrap_chars(&info.display_value(), TOOLTIP_MAX_TEXT_W);
    let line2 = info.source.label();
    let line3 = info
        .description
        .as_ref()
        .map(|d| ellipsize(d, TOOLTIP_MAX_TEXT_W));
    // Padding rows top and bottom, the value rows, the source line, and an
    // optional description line. A value taller than the terminal is cut
    // to fit, the last surviving row ellipsized to say so.
    let fixed = 3 + u16::from(line3.is_some());
    let max_value_rows = screen.height.saturating_sub(fixed).max(1) as usize;
    if value_lines.len() > max_value_rows {
        value_lines.truncate(max_value_rows);
        let last = value_lines.last_mut().unwrap();
        *last = last
            .chars()
            .take(TOOLTIP_MAX_TEXT_W - 1)
            .chain(std::iter::once('\u{2026}'))
            .collect();
    }
    let height = fixed + value_lines.len() as u16;
    let text_w = value_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(line2.chars().count())
        .max(line3.as_ref().map_or(0, |l| l.chars().count())) as u16;
    // 2 columns of padding each side, plus a column for the drop shadow.
    let width = (text_w + 4).min(screen.width.saturating_sub(1));
    if width < 5 || screen.height < height {
        return;
    }
    let below = tip.anchor.bottom();
    let y = if below + height <= screen.bottom() {
        below
    } else {
        tip.anchor.y.saturating_sub(height)
    };
    let x = tip
        .anchor
        .x
        .min(screen.right().saturating_sub(width + 1))
        .max(screen.x);
    let area = Rect::new(x, y, width, height);
    let buf = frame.buffer_mut();
    crate::paint::floating_panel(buf, area, screen, theme);
    let inner = width.saturating_sub(4) as usize;
    let mut row = y + 1;
    for line in &value_lines {
        crate::paint::text(
            buf,
            x + 2,
            row,
            &ellipsize(line, inner),
            theme.text,
            theme.panel,
            true,
        );
        row += 1;
    }
    crate::paint::text(
        buf,
        x + 2,
        row,
        &ellipsize(&line2, inner),
        theme.text_muted,
        theme.panel,
        false,
    );
    if let Some(desc) = &line3 {
        crate::paint::text(
            buf,
            x + 2,
            row + 1,
            &ellipsize(desc, inner),
            theme.text_muted,
            theme.panel,
            false,
        );
    }
}

/// `s` hard-wrapped into chunks of at most `max` characters — values are
/// often unbroken URLs or tokens, so there is no word boundary to prefer.
/// Always yields at least one (possibly empty) line.
fn wrap_chars(s: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(max.max(1))
        .map(|c| c.iter().collect())
        .collect()
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
        let layout =
            crate::layout::compute_layout(ratatui::layout::Rect::new(0, 0, 120, 40), 0.0, 0.0, 0.5);
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
