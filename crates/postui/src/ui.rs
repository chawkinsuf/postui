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

    let project_name = app.display_name();
    let env_label = app.env_label_display();
    let space_label = app.space_name(&app.active_space());
    crate::components::header_bar::draw_header(
        frame,
        layout.header,
        &app.theme,
        &project_name,
        &space_label,
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
                let prepare_ctx = app.prepare_context();
                app.editor.recompute_computed_headers(&prepare_ctx);
            }
            app.editor.env_tls = app.env_tls();
            let hovered = app.hovered.as_ref();
            let dragged_pane = app.drag.as_ref().map(|d| d.pane);
            let modal_open = app.modals.top().is_some();
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
            // The focused jq bar is the one place the terminal's own
            // cursor shows (a bar, so the completion ghost can trail it
            // legibly); everywhere else the caret is painted and the
            // cursor stays hidden. A modal drawn over the pane keeps
            // keyboard focus on the bar underneath, so the cursor is only
            // placed while nothing covers it.
            if !modal_open && let Some(caret) = response.jq_caret_cell() {
                frame.set_cursor_position(caret);
            }
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
                    let App {
                        theme,
                        varmanager,
                        project,
                        hovered,
                        ..
                    } = app;
                    match project.as_ref() {
                        Some(p) => varmanager.draw(
                            frame,
                            body,
                            theme,
                            p,
                            open_request.as_ref(),
                            &mut hits,
                            hovered.as_ref(),
                        ),
                        None => draw_manage_without_a_project(frame, body, theme),
                    }
                }
                tab => {
                    let requests = app.sidebar.space_requests();
                    let App {
                        theme,
                        manage,
                        project,
                        hovered,
                        ..
                    } = app;
                    match project.as_ref() {
                        Some(p) => manage.list.draw(
                            frame,
                            body,
                            theme,
                            tab,
                            p,
                            &requests,
                            &mut hits,
                            hovered.as_ref(),
                        ),
                        None => draw_manage_without_a_project(frame, body, theme),
                    }
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
    // A live row drag takes the footer over completely: it swallows every
    // key but Escape (see `App::handle_key`), so the only chips that mean
    // anything are the two ways to cancel it. The palette chip hides for
    // the same reason it does under a modal — its key is dead — and the
    // quit chip advertises the modified combo, the one key that still
    // pre-empts the drag.
    let drag_live = app.sidebar.drag.is_some() || app.manage.list.drag.is_some();
    let globals_live = modal_chips.is_none() && !drag_live;
    // A plain `q` reaches the quit binding only where nothing consumes it
    // first: no modal (they capture all input), the Main screen (other
    // screens swallow unbound plain keys), and a focus stop that doesn't
    // route typing into a text input — in the editor pane that is only
    // the URL line, the body, and a live cell edit.
    let plain_q_quits = app.modals.is_empty()
        && !drag_live
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
                    .project()
                    .map(|p| app.manage.list.footer_chips(app.manage.tab, p))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(k, l, a)| (k.to_string(), l.to_string(), a))
                    .collect();
            }
            let open_request = app
                .editor
                .slug
                .is_some()
                .then(|| app.editor.current_request());
            app.project()
                .map(|p| app.varmanager.footer_chips(p, open_request.as_ref()))
                .unwrap_or_default()
                .into_iter()
                .map(|(k, l, a)| (k.to_string(), l.to_string(), a))
                .collect()
        })
    });
    let vm_chips = if drag_live {
        Some(vec![
            ("esc".to_string(), "cancel drag".to_string(), None),
            ("right-click".to_string(), "cancel drag".to_string(), None),
        ])
    } else {
        vm_chips
    };
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
        if app.session.response.jq_focused() {
            if app.session.response.jq_menu_open() {
                crate::components::footer::JqBarState::Menu
            } else if app.session.response.jq_ghost().is_some()
                || app.session.response.jq_menu_offered()
            {
                crate::components::footer::JqBarState::Completing {
                    cycle: app.session.response.jq_tab() == crate::config::JqTab::Cycle,
                }
            } else {
                crate::components::footer::JqBarState::Focused
            }
        } else if app.session.response.jq_open() {
            crate::components::footer::JqBarState::Open
        } else {
            crate::components::footer::JqBarState::Closed
        },
        vm_chips,
        globals_live,
        plain_q_quits,
        // The hovered button's hint — the footer stands in for a tooltip.
        app.hovered
            .as_ref()
            .and_then(|h| crate::hint::hint_for(h, &app.keymap, &app.hint_ctx(h)))
            .as_deref(),
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
    match app.var_token_tip() {
        Some(tip) => {
            let info = app.editor.vars.describe(&tip.name);
            // Revealed only for the exact secret the reveal was clicked
            // for: same token *and* same value, so another environment's
            // secret under the same name comes up masked.
            let revealed = matches!(
                (&app.tip_revealed, &info.value),
                (Some((n, v)), Some(value)) if *n == tip.name && v == value
            );
            if !revealed {
                app.tip_revealed = None;
            }
            draw_var_tooltip(
                frame,
                frame.area(),
                &app.theme,
                &tip,
                &info,
                revealed,
                app.hovered.as_ref(),
                &mut app.hits,
            );
            app.drawn_tip = Some(tip);
        }
        None => {
            // No tip this frame: nothing to hold, nothing revealed. A hold
            // that outlived its tip (the token left the screen under a
            // resting pointer) would otherwise re-raise it, secret and
            // all, the moment the token came back.
            app.drawn_tip = None;
            app.tip_held = false;
            app.tip_revealed = None;
        }
    }
}

/// Widest a tooltip line may run before the value wraps onto another row.
const TOOLTIP_MAX_TEXT_W: usize = 56;

/// Draws the hover/caret tooltip for one `{{token}}` (spec §7): first the
/// value — one mask dot per cell for a secret unless `revealed` — wrapped
/// onto further rows rather than truncated so the whole value is
/// readable — then a line naming the scope the value came from (`this
/// request`, `env = qa`, `default`, `option = user 2`, `needs selection`,
/// `missing secret`). When there is a value, icon pills sit at the right
/// of its first row: `󰆏` copy (the real value, a secret's included) and,
/// for a secret, `󰈈` reveal / `󰈉` hide. It sits under the token it
/// belongs to, flipping above when there is no room below, and is
/// clamped to stay inside `screen`; nothing is drawn when even the pills
/// would not fit. Registers the panel and its pills in `hits`.
#[allow(clippy::too_many_arguments)]
fn draw_var_tooltip(
    frame: &mut Frame,
    screen: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
    tip: &crate::app::TokenTip,
    info: &crate::components::var_tokens::TokenInfo,
    revealed: bool,
    hovered: Option<&crate::hit::Hit>,
    hits: &mut crate::hit::HitMap,
) {
    use crate::hit::Hit;
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;
    // Wrap the real value, then mask row by row (one dot per cell of
    // that row): the masked and revealed tips then have the same row
    // structure and widths whatever mix of narrow, wide, or control
    // characters the secret holds, so reveal / hide moves nothing.
    let masked = info.secret && !revealed;
    let mut value_lines: Vec<String> =
        wrap_cells(&info.display_value_unmasked(), TOOLTIP_MAX_TEXT_W)
            .into_iter()
            .map(|row| {
                if masked {
                    "\u{25cf}".repeat(row.width().max(1))
                } else {
                    row
                }
            })
            .collect();
    let line2 = info.source.label();
    let line3 = info
        .description
        .as_ref()
        .map(|d| ellipsize(d, TOOLTIP_MAX_TEXT_W));
    // The icon pills, only when there is a value to act on: three cells
    // each (` 󰆏 `) so the hover fill surrounds the glyph.
    let mut pills: Vec<(&str, Hit)> = Vec::new();
    if info.value.is_some() {
        pills.push((crate::glyph::COPY_PILL, Hit::TipCopy(tip.name.clone())));
        if info.secret {
            let eye = if revealed {
                crate::glyph::EYE_OFF_PILL
            } else {
                crate::glyph::EYE_PILL
            };
            pills.push((eye, Hit::TipReveal(tip.name.clone())));
        }
    }
    // Trailing the first value row: a gap, then the pills.
    let pills_w = 3 * pills.len();
    let controls_w = if pills.is_empty() { 0 } else { 1 + pills_w };
    // Padding rows top and bottom, the value rows, the source line, and an
    // optional description line. A value taller than the terminal is cut
    // to fit, the last surviving row ellipsized to say so.
    let fixed = 3 + u16::from(line3.is_some());
    let max_value_rows = screen.height.saturating_sub(fixed).max(1) as usize;
    if value_lines.len() > max_value_rows {
        value_lines.truncate(max_value_rows);
        let last = value_lines.last_mut().unwrap();
        *last = take_cells(last, TOOLTIP_MAX_TEXT_W - 1) + "\u{2026}";
    }
    let height = fixed + value_lines.len() as u16;
    // Display cells throughout (a wide glyph is one char but two cells),
    // so the pills land after the text rather than on top of its tail.
    let first_value_w = value_lines.first().map_or(0, |l| l.width());
    let text_w = value_lines
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(0)
        .max(first_value_w + controls_w)
        .max(line2.width())
        .max(line3.as_ref().map_or(0, |l| l.width())) as u16;
    // 2 columns of padding each side, plus a column for the drop shadow.
    let width = (text_w + 4).min(screen.width.saturating_sub(1));
    // Too narrow for the padding, the pills, and at least one cell of
    // value before them: no tip at all, rather than pills painted over
    // the border (or placed off a u16 underflow).
    if width < 5 || (width as usize) < 5 + controls_w || screen.height < height {
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
    for (i, line) in value_lines.iter().enumerate() {
        // The first row keeps room for the pills after the value.
        let room = if i == 0 {
            inner.saturating_sub(controls_w)
        } else {
            inner
        };
        crate::paint::text(
            buf,
            x + 2,
            row,
            &ellipsize(line, room),
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
        row += 1;
        crate::paint::text(
            buf,
            x + 2,
            row,
            &ellipsize(desc, inner),
            theme.text_muted,
            theme.panel,
            false,
        );
    }
    // The panel first: later registrations win a hit lookup, so the
    // pills painted next sit on top of it.
    hits.register(area, Hit::TipPanel(tip.name.clone()));
    // The pills, flush with the panel's right padding.
    let mut cx = x + width - 2 - pills_w as u16;
    for (label, hit) in pills {
        let rect = Rect::new(cx, y + 1, 3, 1);
        crate::paint::action(buf, rect, label, hit.clone(), hovered, theme.panel, theme);
        hits.register(rect, hit);
        cx += 3;
    }
}

/// `s` hard-wrapped into lines of at most `max` display cells — values
/// are often unbroken URLs or tokens, so there is no word boundary to
/// prefer. Cells, not chars: a wide glyph takes two, so a masked secret
/// (one dot per cell) and its revealed value wrap identically. Always
/// yields at least one (possibly empty) line.
fn wrap_cells(s: &str, max: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let max = max.max(1);
    let mut lines = vec![String::new()];
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max && w > 0 {
            lines.push(String::new());
            w = 0;
        }
        lines.last_mut().unwrap().push(c);
        w += cw;
    }
    lines
}

/// `s` cut to at most `max` display cells, the last of which becomes `…`.
pub(crate) fn ellipsize(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if s.width() <= max {
        return s.to_string();
    }
    take_cells(s, max.saturating_sub(1)) + "\u{2026}"
}

/// The longest prefix of `s` that fits in `max` display cells.
pub(crate) fn take_cells(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > max {
            break;
        }
        out.push(c);
        w += cw;
    }
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

/// The Manage screen's body with no project open. Both tabs are built
/// around `&Project`, so there is nothing to list — but the screen is
/// reachable (`Action::OpenManage` is not gated), and an unpainted body
/// would show raw terminal default where the themed page belongs. Paints
/// the page and says why it is empty, in the same words the write gate
/// uses.
fn draw_manage_without_a_project(
    frame: &mut ratatui::Frame,
    body: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
) {
    use crate::paint::{fill, text};
    fill(frame.buffer_mut(), body, theme.page);
    const MSG: &str = "no project is open \u{2014} open or create one first";
    let x = body.x + body.width.saturating_sub(MSG.chars().count() as u16) / 2;
    let y = body.y + body.height / 3;
    text(frame.buffer_mut(), x, y, MSG, theme.text_muted, theme.page, false);
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
        assert!(!content.contains("postui")); // no wordmark: the window title carries the name
        assert!(content.contains("Project:")); // header project chip
        assert!(content.contains("no env")); // header env chip: a bare dir has no envs
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
