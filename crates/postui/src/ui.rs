use crate::app::{App, Screen};
use crate::components::footer::FooterChip;
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
        if app.session.response.collapsed { 1.0 } else { 0.0 },
    );
    let ratio_t =
        app.anims.value_or(crate::anim::AnimKey::SplitRatio, now, app.split_ratio.editor_share());
    let layout = compute_layout(frame.area(), collapse_t, response_t, ratio_t);
    let focus = app.focus;
    let screen = app.screen;
    let mut hits = std::mem::take(&mut app.hits);
    hits.clear();

    let project_name = app.display_name();
    let env_label = app.env_label_display();
    let space_label = app.space_name(&app.active_space());
    // The shared hover fade, sampled once for the whole frame: every
    // hovered keycap and chip on the bar and in the footer warms through
    // it, and it restarts whenever the pointer crosses onto another hit.
    let hover_t = app.anims.value_or(crate::anim::AnimKey::Hover, now, 1.0);
    let (save_key, discard_key) = header_keys(app);
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
        // Reload takes the same slot whenever the Manage screen is up —
        // the save/discard group above requires Screen::Main, so the two
        // never both show.
        screen == Screen::Manage,
        &save_key,
        &discard_key,
        &mut hits,
        app.hovered.as_ref(),
        hover_t,
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
            // other tab, so skip building the context and its one pass.
            if app.editor.active_tab == crate::components::editor::EditorTab::Headers {
                let prepare_ctx = app.prepare_context();
                app.editor.recompute_computed_headers(&prepare_ctx);
            }
            app.editor.env_tls = app.env_tls();
            app.vim_mirror_body_search();
            let hovered = app.hovered.as_ref();
            let pointer = app.pointer;
            let dragged_pane = app.drag.as_ref().map(|d| d.pane);
            let modal_open = app.modals.top().is_some();
            // Destructured so each component can be borrowed mutably
            // alongside the shared theme reference its DrawCtx holds.
            let App { theme, sidebar, editor, session, anims, .. } = app;
            let response = &mut session.response;
            let ctx = |pane: PaneId| DrawCtx {
                theme,
                focused: focus == pane,
                hovered,
                pointer,
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
            // Recorded so `App::retarget_manage_tab_underline` (run from
            // `update`, off the draw path) can lay the strip out with the
            // same right-anchored geometry `draw_manage_bar` below will
            // actually paint. Both read it from `manage::strip_area`, so
            // "the strip's width" has exactly one definition.
            app.manage_strip_width = crate::components::manage::strip_area(bar).width;
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
                let left = app.anims.value(AnimKey::TabUnderline(StripId::ManageTabs), now);
                let right = app.anims.value(AnimKey::TabUnderlineWidth(StripId::ManageTabs), now);
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
                    let open_request =
                        app.editor.slug.is_some().then(|| app.editor.current_request());
                    let App { theme, varmanager, project, hovered, .. } = app;
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
                crate::components::manage::ManageTab::Settings => {
                    // The one tab that isn't about the open project: it
                    // renders regardless, which is why it is a match arm
                    // of its own rather than a branch of the no-project
                    // message the other three share.
                    crate::components::settings::draw_settings(
                        frame,
                        body,
                        &app.theme,
                        &app.settings,
                        &app.ui_settings,
                        app.config_error.as_deref(),
                        &mut hits,
                        app.hovered.as_ref(),
                    );
                }
                tab => {
                    let requests = app.sidebar.space_requests();
                    let App { theme, manage, project, hovered, .. } = app;
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
                pointer: app.pointer,
                dragging: false,
                anims: &app.anims,
                now,
            };
            crate::components::testbed::draw_testbed(frame, layout.body, &ctx);
        }
    }

    // The palette chip hides under a modal with chips of its own and
    // during a live row drag, for the same reason: its key is dead. The
    // quit chip then advertises the modified combo, the one key that
    // still pre-empts both.
    let drag_live = drag_live(app);
    let globals_live = app.modals.footer_chips().is_none() && !drag_live;
    // A plain `q` reaches the quit binding only in the arrows profile (vim
    // gave `q` up, plan M1 Deviation 4) and only where nothing consumes it
    // first: no modal (they capture all input), the Main screen (other
    // screens swallow unbound plain keys), and a focus stop that doesn't
    // route typing into a text input — in the editor pane that is only
    // the URL line, the body, and a live cell edit.
    let plain_q_quits = app.profile() == crate::config::KeyProfile::Arrows
        && app.modals.is_empty()
        && !drag_live
        && (app.screen == Screen::Main
            && (matches!(focus, PaneId::Sidebar | PaneId::Response)
                || focus == PaneId::Editor && !app.editor.plain_keys_type())
            // On the Manage screens the keymap's `q` reaches Quit through
            // the screen whitelist; only a live edit types it — the
            // Variables tab's form and grid cells, and the Settings tab's
            // field rows.
            || app.screen == Screen::Manage
                && match app.manage.tab {
                    crate::components::manage::ManageTab::Variables => {
                        app.varmanager.form.editing.is_none()
                            && app.varmanager.grid.editing.is_none()
                    }
                    crate::components::manage::ManageTab::Settings => {
                        app.settings.editing.is_none()
                    }
                    _ => true,
                });
    let vm_chips = override_chips(app);
    let pane = PaneChipArgs::of(app);
    let vim_status = app.vim_status();
    crate::components::footer::draw_footer(
        frame,
        layout.footer,
        &app.theme,
        pane.focus,
        pane.shift_enter_send,
        pane.sending,
        pane.add_row_label,
        pane.url_focused,
        pane.url_selected,
        pane.table_row_selected,
        pane.jq_bar,
        pane.field_open,
        vm_chips,
        globals_live,
        plain_q_quits,
        vim_status.as_deref(),
        app.spell_ctx(),
        &app.keymap,
        // The hovered button's hint — the footer stands in for a tooltip —
        // or what is wrong with a client-owned header row.
        app.footer_hint().as_ref(),
        &mut hits,
        app.hovered.as_ref(),
        hover_t,
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
        height: frame.area().height.saturating_sub(crate::components::footer::FOOTER_HEIGHT),
        ..frame.area()
    };
    app.toasts.draw(frame, toast_area, &app.theme, &app.anims, now);
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

/// Widest a tooltip line may run before the value or description wraps
/// onto another row.
const TOOLTIP_MAX_TEXT_W: usize = 56;

/// Draws the hover/caret tooltip for one `{{token}}` (spec §7). Three
/// parts, top to bottom, a blank row between each:
///
/// - the **value**, when there is one — one mask dot per cell for a
///   secret unless `revealed` — wrapped onto further rows rather than
///   truncated so the whole value is readable;
/// - the **name** row: the variable's name in the token's own tint, and
///   the scope its value came from in muted text (`this request`,
///   the environment as the header chip names it (`qa`, or `no env` for a
///   secret kept under no environment), `default value`, the selected
///   option's name (`user 2`), `needs selection`,
///   `missing secret`, `not defined`);
/// - the **descriptions**, when written, each word-wrapped in muted text
///   and stacked (a selector field's selector's, then its selected
///   option's, see
///   [`crate::components::var_tokens::TokenInfo::descriptions`]).
///
/// The top row — the value's first, or the name row when there is no
/// value — carries the icon pills right-aligned: `󰈈` reveal / `󰈉` hide
/// for a secret, `󰆏` copy (the real value, a secret's included) when
/// there is a value, and `󰏫` edit always. Right-anchored in that order:
/// edit is the primary verb and holds the anchor edge, reveal is the
/// rare, dynamic one and comes and goes at the far end without moving the
/// other two. Rows wrap to [`TOOLTIP_MAX_TEXT_W`] or the screen's room,
/// whichever is narrower; the value's a pill strip narrower, so its rows
/// stay clear of the pills.
///
/// It sits under the token it belongs to, or above when it doesn't fit
/// below and there is more room above, cut to fit the roomier side so it
/// never covers the token, and clamped to stay inside `screen`; nothing
/// is drawn when not even one row of each section fits. Registers the
/// panel and its pills in `hits`.
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
    // The icon pills: three cells each (` 󰆏 `) so the hover fill
    // surrounds the glyph.
    let mut pills: Vec<(&str, Hit)> = Vec::new();
    if info.secret && info.value.is_some() {
        let eye = if revealed { crate::glyph::EYE_OFF_PILL } else { crate::glyph::EYE_PILL };
        pills.push((eye, Hit::TipReveal(tip.name.clone())));
    }
    if info.value.is_some() {
        pills.push((crate::glyph::COPY_PILL, Hit::TipCopy(tip.name.clone())));
    }
    pills.push((crate::glyph::PENCIL_PILL, Hit::TipEdit(tip.name.clone())));
    let pills_w = 3 * pills.len();
    // Rows wrap to the max width, or what the screen has room for inside
    // the padding and the drop shadow. The top row carries the pills, so
    // the value — whose block starts there — wraps a pill strip (and a
    // cell of gap) narrower, every row alike, so the pills sit in a
    // column of their own.
    let wrap_w = TOOLTIP_MAX_TEXT_W.min(screen.width.saturating_sub(5) as usize).max(1);
    let top_w = wrap_w.saturating_sub(1 + pills_w).max(1);
    // Wrap the real value, then mask row by row (one dot per cell of
    // that row): the masked and revealed tips then have the same row
    // structure and widths whatever mix of narrow, wide, or control
    // characters the secret holds, so reveal / hide moves nothing.
    let masked = info.secret && !revealed;
    let mut value_lines: Vec<String> = match &info.value {
        Some(value) => wrap_cells(value, top_w)
            .into_iter()
            .map(|row| if masked { "\u{25cf}".repeat(cells(&row).max(1)) } else { row })
            .collect(),
        None => Vec::new(),
    };
    let source = info.source.label();
    // The descriptions stack, each wrapped on its own and starting a row
    // of its own.
    let mut descs: Vec<Vec<String>> = info
        .descriptions
        .iter()
        .map(|d| wrap_words(d, wrap_w))
        .filter(|lines| !lines.is_empty())
        .collect();
    // The name row: the name, two cells, then the source. With no value
    // it is the top row, beside the pills.
    let has_value = !value_lines.is_empty();
    let name_w =
        (cells(&tip.name) + 2 + cells(&source)).min(if has_value { wrap_w } else { top_w });
    // The tip goes below its token when it fits there, else above when
    // there is more room above; it is cut to the roomier side, so it
    // never covers the token it describes. It sits flush against the
    // token on both sides: the pointer must cross from token to panel
    // without leaving either, or the tip closes before its pills can be
    // reached (so above, its drop shadow falls on the token's row).
    let below_room = screen.bottom().saturating_sub(tip.anchor.bottom());
    let above_room = tip.anchor.y.saturating_sub(screen.y);
    let side_room = below_room.max(above_room);
    // Top to bottom: the value (when there is one), the name row, the
    // descriptions (when written), a blank row between each part, padding
    // rows top and bottom. With too little room for it all, the value
    // gives up rows first, then the descriptions — the first in the stack
    // before the later, more specific ones — the last surviving row of a
    // cut one ellipsized to say so. Each keeps at least one row: with no
    // room for that, there is no tip at all.
    let has_desc = !descs.is_empty();
    let fixed = 3 + usize::from(has_value) + usize::from(has_desc);
    let room = (side_room as usize).saturating_sub(fixed);
    let desc_len: usize = descs.iter().map(Vec::len).sum();
    cut_rows(&mut value_lines, room.saturating_sub(desc_len).max(1), top_w);
    let mut desc_room = room.saturating_sub(value_lines.len()).max(descs.len());
    for (i, lines) in descs.iter_mut().enumerate().rev() {
        // Leave a row for each description before this one.
        let keep = lines.len().min(desc_room.saturating_sub(i));
        cut_rows(lines, keep, wrap_w);
        desc_room -= lines.len();
    }
    let desc_lines = descs.concat();
    // Display cells throughout (a wide glyph is one char but two cells).
    let widest = |lines: &[String]| lines.iter().map(|l| cells(l)).max().unwrap_or(0);
    // At least one cell, as a mask always shows one dot: an empty value
    // revealed keeps its masked footprint (and the guard below its tip).
    let top_text_w = if has_value { widest(&value_lines) } else { name_w }.max(1);
    let below_top_w = widest(&desc_lines).max(if has_value { name_w } else { 0 });
    let text_w = (top_text_w + 1 + pills_w).max(below_top_w) as u16;
    let mut rows: Vec<TipRow> = value_lines.into_iter().map(TipRow::Value).collect();
    if has_value {
        rows.push(TipRow::Gap);
    }
    rows.push(TipRow::Name);
    if has_desc {
        rows.push(TipRow::Gap);
        rows.extend(desc_lines.into_iter().map(TipRow::Description));
    }
    let height = rows.len() as u16 + 2;
    // 2 columns of padding each side, plus a column for the drop shadow.
    let width = (text_w + 4).min(screen.width.saturating_sub(1));
    // Too narrow for the padding, the pills, and at least one cell of the
    // top row's text before them: no tip at all, rather than pills painted
    // over the border (or placed off a u16 underflow).
    if (width as usize) < 6 + pills_w || side_room < height {
        return;
    }
    let y = if height <= below_room { tip.anchor.bottom() } else { tip.anchor.y - height };
    let x = tip.anchor.x.min(screen.right().saturating_sub(width + 1)).max(screen.x);
    let area = Rect::new(x, y, width, height);
    let buf = frame.buffer_mut();
    crate::paint::floating_panel(buf, area, screen, theme);
    let inner = width.saturating_sub(4) as usize;
    for (i, tip_row) in rows.iter().enumerate() {
        let row = y + 1 + i as u16;
        // The top row keeps room for the pills after its text.
        let room = if i == 0 { inner.saturating_sub(1 + pills_w) } else { inner };
        let (text, fg) = match tip_row {
            TipRow::Value(line) => (line, theme.text),
            TipRow::Description(line) => (line, theme.text_muted),
            TipRow::Gap => continue,
            TipRow::Name => {
                // The name in the token's tint (so the panel reads as
                // that token's), the source muted after it — cut from the
                // source first, the name last.
                let name = ellipsize(&tip.name, room);
                let source_room = room.saturating_sub(cells(&name) + 2);
                let name_end = crate::paint::text(
                    buf,
                    x + 2,
                    row,
                    &name,
                    crate::components::var_tokens::token_color(theme, info),
                    theme.panel,
                    true,
                );
                if source_room > 0 {
                    crate::paint::text(
                        buf,
                        name_end + 2,
                        row,
                        &ellipsize(&source, source_room),
                        theme.text_muted,
                        theme.panel,
                        false,
                    );
                }
                continue;
            }
        };
        crate::paint::text(buf, x + 2, row, &ellipsize(text, room), fg, theme.panel, false);
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

/// One row of the variable tooltip, top to bottom.
enum TipRow {
    Value(String),
    /// The variable's name and where its value came from.
    Name,
    Description(String),
    /// The blank row between two sections.
    Gap,
}

/// `s` hard-wrapped into lines of at most `max` display cells — values
/// are often unbroken URLs or tokens, so there is no word boundary to
/// prefer. Cells, not chars: a wide glyph takes two, so a masked secret
/// (one dot per cell) and its revealed value wrap identically. Breaks
/// only between graphemes. Always yields at least one (possibly empty)
/// line.
fn wrap_cells(s: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = vec![String::new()];
    let mut w = 0;
    for (_, g, gw) in graphemes(s) {
        if w + gw > max && w > 0 {
            lines.push(String::new());
            w = 0;
        }
        lines.last_mut().unwrap().push_str(g);
        w += gw;
    }
    lines
}

/// Prose `s` as it reads on one row: every run of whitespace, newlines
/// included, folded to one space, none at either end. The one rule every
/// description drawer follows — [`wrap_words`] folds the same way.
pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Prose `s` wrapped into lines of at most `max` display cells, breaking
/// between words; whitespace folds as [`one_line`] folds it. A word wider
/// than a line is hard-broken as [`wrap_cells`] does. A blank `s` yields
/// no lines at all.
fn wrap_words(s: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for piece in s.split_whitespace().flat_map(|word| wrap_cells(word, max)) {
        if !line.is_empty() && cells(&line) + 1 + cells(&piece) > max {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&piece);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Cuts `lines` to at most `rows`, the last survivor ellipsized (within
/// `max` cells) to say the rest is missing.
fn cut_rows(lines: &mut Vec<String>, rows: usize, max: usize) {
    if lines.len() > rows {
        lines.truncate(rows);
        if let Some(last) = lines.last_mut() {
            *last = format!("{}\u{2026}", take_cells(last, max.saturating_sub(1)));
        }
    }
}

/// `s` cut to at most `max` display cells, the last of which becomes `…`.
pub(crate) fn ellipsize(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    format!("{}\u{2026}", take_cells(s, max - 1))
}

/// `ellipsize` from the other end: `s` cut to at most `max` display
/// cells, the first of which becomes `…`, so its end stays in view.
pub(crate) fn ellipsize_start(s: &str, max: usize) -> String {
    if cells(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut w = 0;
    let mut start = s.len();
    for (i, _, gw) in graphemes(s).rev() {
        w += gw;
        if w > max - 1 {
            break;
        }
        start = i;
    }
    format!("\u{2026}{}", &s[start..])
}

/// The longest prefix of `s` that fits in `max` display cells, cut
/// between graphemes. A wide glyph that would straddle the edge is left
/// out.
pub(crate) fn take_cells(s: &str, max: usize) -> &str {
    let mut w = 0;
    for (i, _, gw) in graphemes(s) {
        w += gw;
        if w > max {
            return &s[..i];
        }
    }
    s
}

/// [`take_cells`] for a `u16` room, the width a layout hands a painter.
pub(crate) fn clip(s: &str, width: u16) -> &str {
    take_cells(s, width as usize)
}

/// The display cells `s` takes on screen, for sizing a label and placing
/// what follows it: the crate's width measure, by grapheme as ratatui
/// paints. Only code that must count per char like another program
/// measures for itself — the body editor as edtui lays text out, the vim
/// engine and the response view's Vim cursor as Vim counts columns;
/// `lib.rs`'s `width_measure_test` holds the rest to this.
pub(crate) fn cells(s: &str) -> usize {
    // ASCII graphemes are one byte each (`\r\n` aside, two controls): a
    // cell apiece, none for a control.
    if s.is_ascii() {
        return s.bytes().filter(|b| !b.is_ascii_control()).count();
    }
    graphemes(s).map(|(_, _, w)| w).sum()
}

/// `s`'s graphemes, each with its byte offset and the cells it takes —
/// the unit ratatui paints in: its own width for the grapheme, and none
/// for one holding a control char, which it skips.
pub(crate) fn graphemes(s: &str) -> Graphemes<'_> {
    use unicode_segmentation::GraphemeCursor;
    let n = s.len();
    Graphemes {
        s,
        front: 0,
        back: n,
        fwd: GraphemeCursor::new(0, n, true),
        bwd: GraphemeCursor::new(n, n, true),
    }
}

/// One grapheme of [`char_graphemes`].
pub(crate) struct CharGrapheme<'a> {
    /// The char offset it starts at, and how many chars it holds.
    pub char: usize,
    pub chars: usize,
    pub byte: usize,
    pub text: &'a str,
    /// The cells it paints, as [`graphemes`] gives them.
    pub width: usize,
}

/// [`graphemes`] for text indexed by char — a cursor, a selection, a
/// search hit — with each grapheme's char offset and char count beside
/// its byte offset.
pub(crate) fn char_graphemes(s: &str) -> impl Iterator<Item = CharGrapheme<'_>> {
    let mut char = 0;
    graphemes(s).map(move |(byte, text, width)| {
        let chars = if text.len() == 1 { 1 } else { text.chars().count() };
        char += chars;
        CharGrapheme { char: char - chars, chars, byte, text, width }
    })
}

/// [`graphemes`]' iterator. It hands the segmenter only what can need it:
/// two ASCII chars always have a grapheme boundary between them but for
/// `\r\n` (every UAX #29 rule that joins two chars needs a non-ASCII one),
/// so an ASCII char with ASCII beside it is a grapheme alone. That keeps
/// a megabyte line with a little non-ASCII in it near the cost of a char
/// walk (task #45). The rest goes to one segmenter cursor per end, kept
/// across steps, so a run of non-ASCII costs what `grapheme_indices` does.
pub(crate) struct Graphemes<'a> {
    s: &'a str,
    /// The unvisited bytes, both ends on grapheme boundaries.
    front: usize,
    back: usize,
    fwd: unicode_segmentation::GraphemeCursor,
    bwd: unicode_segmentation::GraphemeCursor,
}

impl<'a> Graphemes<'a> {
    fn item(&self, at: usize, len: usize) -> (usize, &'a str, usize) {
        use ratatui::buffer::CellWidth;
        let g = &self.s[at..at + len];
        let w = match g.as_bytes() {
            [b] if b.is_ascii() => usize::from(!b.is_ascii_control()),
            _ if g.contains(char::is_control) => 0,
            _ => g.cell_width() as usize,
        };
        (at, g, w)
    }
}

/// Whether ASCII `a` then ASCII `b` have a grapheme boundary between them.
fn ascii_break(a: u8, b: u8) -> bool {
    a.is_ascii() && b.is_ascii() && !(a == b'\r' && b == b'\n')
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = (usize, &'a str, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.s[self.front..self.back];
        let len = match rest.as_bytes() {
            [] => return None,
            [a] if a.is_ascii() => 1,
            [a, b, ..] if ascii_break(*a, *b) => 1,
            _ => {
                if self.fwd.cur_cursor() != self.front {
                    self.fwd.set_cursor(self.front);
                }
                let next = self.fwd.next_boundary(self.s, 0);
                next.expect("the whole string is the chunk")? - self.front
            }
        };
        let at = self.front;
        self.front += len;
        Some(self.item(at, len))
    }
}

impl DoubleEndedIterator for Graphemes<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let rest = &self.s[self.front..self.back];
        let len = match rest.as_bytes() {
            [] => return None,
            [b] if b.is_ascii() => 1,
            [.., a, b] if ascii_break(*a, *b) => 1,
            _ => {
                if self.bwd.cur_cursor() != self.back {
                    self.bwd.set_cursor(self.back);
                }
                let prev = self.bwd.prev_boundary(self.s, 0);
                self.back - prev.expect("the whole string is the chunk")?
            }
        };
        self.back -= len;
        Some(self.item(self.back, len))
    }
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

/// The Manage screen's body with no project open, for the project-scoped
/// tabs — Variables, Environments and Spaces are all built around
/// `&Project`, so there is nothing to list.
/// Settings is exempt: it renders its own body regardless (see the
/// `ManageTab::Settings` match arm above), which is also why the screen is
/// reachable with no project at all (`Action::OpenManage` is not gated).
/// Paints the page and says why it is empty, in the same words the write
/// gate uses.
fn draw_manage_without_a_project(
    frame: &mut ratatui::Frame,
    body: ratatui::layout::Rect,
    theme: &crate::theme::Theme,
) {
    use crate::paint::{fill, text};
    fill(frame.buffer_mut(), body, theme.page);
    const MSG: &str = "no project is open \u{2014} open or create one first";
    let x = body.x + body.width.saturating_sub(cells(MSG) as u16) / 2;
    let y = body.y + body.height / 3;
    text(frame.buffer_mut(), x, y, MSG, theme.text_muted, theme.page, false);
}

/// The header bar's save and discard keycaps (plan M4b Deviation 20),
/// spelled on the screen's own surface with no field: the key that works
/// once any field is left and any dialog answered, so a button never
/// paints blank (`:w` in vim even while a dialog, where `:` does not
/// apply, is up).
pub(crate) fn header_keys(app: &App) -> (String, String) {
    use crate::action::Action;
    use crate::keyroute::spell::{Key, keycap};
    let at = crate::keyroute::SpellCtx {
        profile: app.profile(),
        surface: app.screen_surface(),
        field: crate::keyroute::FieldSpell::None,
    };
    let key = |a: Action| keycap(&Key::Of(a), at, &app.keymap).unwrap_or_default();
    (key(Action::SaveRequest), key(Action::DiscardChanges))
}

/// A row drag is live in the sidebar or a Manage list.
fn drag_live(app: &App) -> bool {
    app.sidebar.drag.is_some() || app.manage.list.drag.is_some()
}

/// The chips that replace the main panes' wholesale, or `None` for the
/// panes' own ([`PaneChipArgs`]). The Manage screen swaps in its own chip
/// set — the per-pane chips' actions target requests, which aren't on
/// screen there. An open modal with chips of its own wins over both:
/// while it captures the keyboard, its quick actions are the only ones
/// that work. A live row drag takes the footer over completely: it
/// swallows every key but Escape (see `App::handle_key`), so the only
/// chips that mean anything are the two ways to cancel it.
pub(crate) fn override_chips(app: &App) -> Option<Vec<FooterChip>> {
    use crate::keyroute::spell::Key;
    if drag_live(app) {
        return Some(vec![
            (Key::Named("esc"), "cancel drag".to_string(), None),
            (Key::Named("right-click"), "cancel drag".to_string(), None),
        ]);
    }
    app.modals.footer_chips().or_else(|| {
        (app.screen == Screen::Manage).then(|| {
            let mut chips: Vec<FooterChip> = manage_tab_chips(app);
            // The tab strip belongs to the shell, not to any tab, and its
            // chord runs above every tab body's keys (`handle_key_routed`),
            // so it is the one chip every Manage tab shares -- last, so
            // nothing shifts as the tab's own chips come and go.
            chips.push((
                crate::components::footer::tabs_key(),
                "tabs".to_string(),
                Some(crate::action::Action::CycleTabs(1)),
            ));
            chips
        })
    })
}

/// The footer's left-hand chips, unspelled, exactly as `draw` hands them
/// to `draw_footer`: the override if any, else the panes' own.
#[cfg(test)]
pub(crate) fn footer_chips_of(app: &App) -> Vec<FooterChip> {
    override_chips(app).unwrap_or_else(|| PaneChipArgs::of(app).chips())
}

/// What the main panes' footer chips are built from
/// (`footer::footer_chips`), read off the app in one place: `draw` hands
/// it to the footer, and the key matrix's spelling test builds the same
/// chips from it, so the two cannot drift.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneChipArgs {
    pub focus: PaneId,
    pub shift_enter_send: bool,
    pub sending: bool,
    pub add_row_label: Option<&'static str>,
    pub url_focused: bool,
    pub url_selected: bool,
    pub table_row_selected: Option<(usize, bool)>,
    pub jq_bar: crate::components::footer::JqBarState,
    pub field_open: bool,
}

impl PaneChipArgs {
    pub(crate) fn of(app: &App) -> Self {
        use crate::components::editor::SubFocus;
        use crate::components::footer::JqBarState;
        let focus = app.focus;
        let response = &app.session.response;
        Self {
            focus,
            shift_enter_send: app.shift_enter_send,
            sending: app.editor.sending,
            // No add chip while a new row is already mid-add (the ghost-row
            // edit); editing an existing row keeps it.
            add_row_label: (!app.editor.adding_row())
                .then(|| app.editor.active_tab.add_row_label())
                .flatten(),
            url_focused: matches!(app.editor.sub_focus, SubFocus::Method | SubFocus::Url),
            // Precisely the URL line, selected (not open) — the "enter edit"
            // chip's own gate. `url_focused` covers the whole address bar
            // (method or URL), which is too broad for a chip that names
            // `Action::FocusUrl`: the method badge's Enter opens the method
            // dropdown, not the URL line.
            url_selected: app.editor.sub_focus == SubFocus::Url && !app.editor.url_open(),
            // A selected data row (content focus, no cell edit — space/d
            // would type into one) advertises its toggle/delete keys.
            table_row_selected: (focus == PaneId::Editor
                && app.editor.sub_focus == SubFocus::Content
                && app.editor.table.editing.is_none())
            .then(|| {
                app.editor
                    .table
                    .selected
                    .filter(|s| *s < app.editor.table_len())
                    .map(|i| (i, app.editor.table_row_enabled(i)))
            })
            .flatten(),
            jq_bar: if response.jq_focused() {
                if response.jq_bar().ai_asking.is_some() {
                    JqBarState::Describing
                } else if response.jq_menu_open() {
                    JqBarState::Menu
                } else if response.jq_ghost_is_closer() {
                    JqBarState::Closer
                } else if response.jq_ghost().is_some() || response.jq_menu_offered() {
                    JqBarState::Completing {
                        cycle: response.jq_tab() == crate::config::JqTab::Cycle,
                    }
                } else {
                    JqBarState::Focused
                }
            } else if response.jq_open() {
                JqBarState::Open
            } else {
                JqBarState::Closed
            },
            // A text field of this pane's own owns Esc: it says `esc done`.
            field_open: app.pane_field_open(focus),
        }
    }

    /// The per-pane chips these arguments paint, unspelled: what
    /// `draw_footer` builds before a modal's or a Manage tab's override.
    #[cfg(test)]
    pub(crate) fn chips(self) -> Vec<FooterChip> {
        crate::components::footer::footer_chips(
            self.focus,
            self.shift_enter_send,
            self.sending,
            self.add_row_label,
            self.url_focused,
            self.url_selected,
            self.table_row_selected,
            self.jq_bar,
            self.field_open,
        )
        .into_iter()
        .map(|(k, l, a)| (k, l.to_string(), a))
        .collect()
    }
}

/// The Manage screen's per-tab chips: the tab body's own keys.
pub(crate) fn manage_tab_chips(app: &App) -> Vec<FooterChip> {
    // Settings publishes its own chips and works with no project open, so
    // it is answered before the project-scoped tabs.
    if app.manage.tab == crate::components::manage::ManageTab::Settings {
        return app
            .settings
            .footer_chips()
            .into_iter()
            .map(|(k, l, a)| (k, l.to_string(), a))
            .collect();
    }
    if app.manage.tab != crate::components::manage::ManageTab::Variables {
        return app
            .project()
            .map(|p| app.manage.list.footer_chips(app.manage.tab, p))
            .unwrap_or_default()
            .into_iter()
            .map(|(k, l, a)| (k, l.to_string(), a))
            .collect();
    }
    let open_request = app.editor.slug.is_some().then(|| app.editor.current_request());
    app.project()
        .map(|p| app.varmanager.footer_chips(p, open_request.as_ref()))
        .unwrap_or_default()
        .into_iter()
        .map(|(k, l, a)| (k, l.to_string(), a))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::var_tokens::{TokenInfo, TokenSource};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;

    /// Task #45: the ASCII shortcut in `graphemes` cuts exactly where the
    /// segmenter does, from either end — around `\r\n`, a base char with a
    /// combining mark or a keycap after it, a prepended mark before ASCII,
    /// flags, ZWJ families and skin tones.
    #[test]
    fn graphemes_cut_where_the_segmenter_does() {
        use unicode_segmentation::UnicodeSegmentation;
        let samples = [
            "",
            "a",
            "ab\r\ncd\r",
            "\r\n",
            "cafe\u{301}x",
            "1\u{FE0F}\u{20E3}#",
            "x\u{600}1y",
            "\u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}a",
            "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b",
            "\u{1F44B}\u{1F3FD}y\u{2764}\u{FE0F}",
            "日本語 tab\there",
        ];
        for s in samples {
            let want: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
            let got: Vec<(usize, &str)> = graphemes(s).map(|(i, g, _)| (i, g)).collect();
            assert_eq!(got, want, "forward {s:?}");
            let mut back: Vec<(usize, &str)> = graphemes(s).rev().map(|(i, g, _)| (i, g)).collect();
            back.reverse();
            assert_eq!(back, want, "backward {s:?}");
            // Both ends at once meet in the middle without a gap.
            let mut it = graphemes(s);
            let mut ends = Vec::new();
            while let Some((i, g, _)) = it.next() {
                ends.push((i, g));
                if let Some((i, g, _)) = it.next_back() {
                    ends.push((i, g));
                }
            }
            ends.sort();
            assert_eq!(ends, want, "both ends {s:?}");
        }
    }

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    /// A cut is measured in cells: a 2-cell glyph that would straddle the
    /// edge is left out, so the cut never paints past its room.
    #[test]
    fn take_cells_cuts_in_cells_and_leaves_out_a_straddling_glyph() {
        assert_eq!(take_cells("本番環境", 5), "本番");
        assert_eq!(take_cells("本番環境", 4), "本番");
        assert_eq!(take_cells("本番環境", 0), "");
        assert_eq!(take_cells("abcdef", 4), "abcd");
        assert_eq!(take_cells("abc", 10), "abc");
    }

    /// Text the crate measures by grapheme, as ratatui paints it: an emoji
    /// with a presentation selector, combining marks, ZWJ sequences, wide
    /// and halfwidth glyphs, and a control char (which paints nothing).
    const GRAPHEME_SAMPLES: [&str; 9] = [
        "a\r\nb\u{7f}c",
        "\u{2764}\u{fe0f}",
        "e\u{301}\u{302}",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        "本番",
        "\u{ff76}\u{ff9e}",
        "a\tb",
        "\u{1f1fa}\u{1f1f8}",
        "x\u{200b}y",
    ];

    /// `cells` is the paint's measure: what `set_stringn` advances by.
    #[test]
    fn cells_measures_what_ratatui_paints() {
        for s in GRAPHEME_SAMPLES {
            let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 40, 1));
            let (end, _) = buf.set_stringn(0, 0, s, 40, ratatui::style::Style::default());
            assert_eq!(cells(s), end as usize, "{s:?}");
        }
        assert_eq!(cells("\u{2764}\u{fe0f}"), 2);
        assert_eq!(cells("e\u{301}"), 1);
    }

    /// A cut keeps whole graphemes: a presentation-selector emoji is two
    /// cells, and a mark stays with its letter.
    #[test]
    fn take_cells_keeps_whole_graphemes() {
        let hearts = "\u{2764}\u{fe0f}\u{2764}\u{fe0f}\u{2764}\u{fe0f}";
        assert_eq!(take_cells(hearts, 3), "\u{2764}\u{fe0f}");
        assert_eq!(take_cells("e\u{301}x", 1), "e\u{301}");
        assert_eq!(take_cells("ae\u{301}", 1), "a");
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        assert_eq!(take_cells(&format!("{family}{family}"), 3), family);
    }

    /// The front cut keeps whole graphemes too: a mark never survives
    /// without the glyph it sits on.
    #[test]
    fn ellipsize_start_never_keeps_a_mark_without_its_base() {
        assert_eq!(ellipsize_start("x本\u{301}", 2), "\u{2026}");
        assert_eq!(ellipsize_start("xye\u{301}", 2), "\u{2026}e\u{301}");
        assert_eq!(ellipsize("\u{2764}\u{fe0f}\u{2764}\u{fe0f}", 3), "\u{2764}\u{fe0f}\u{2026}");
    }

    /// A hard wrap breaks between graphemes, never inside a ZWJ sequence,
    /// and measures a presentation-selector emoji as its two cells.
    #[test]
    fn wrap_cells_breaks_between_graphemes() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        assert_eq!(wrap_cells(&format!("{family}{family}"), 3), vec![family, family]);
        let heart = "\u{2764}\u{fe0f}";
        assert_eq!(wrap_cells(&heart.repeat(3), 3), vec![heart; 3]);
        assert_eq!(wrap_words(&format!("{heart} {heart}"), 4), vec![heart, heart]);
    }

    /// No room, no text: not even the `…`, which would take a cell the
    /// caller doesn't have.
    #[test]
    fn ellipsizing_to_zero_cells_gives_nothing() {
        assert_eq!(ellipsize("abc", 0), "");
        assert_eq!(ellipsize_start("abc", 0), "");
        assert_eq!(cells("本番ab"), 6);
    }

    /// The mirror of `ellipsize`: the end survives behind a leading `…`,
    /// within `max` cells.
    #[test]
    fn ellipsize_start_keeps_the_end_within_its_cells() {
        assert_eq!(ellipsize_start("/home/user/project", 30), "/home/user/project");
        assert_eq!(ellipsize_start("/home/user/project", 8), "\u{2026}project");
        let cut = ellipsize_start("/home/本番環境/作業", 8);
        assert!(cut.width() <= 8, "{cut:?} is {} cells", cut.width());
        assert_eq!(cut, "\u{2026}境/作業");
    }

    /// A plain token's info, its value from the `qa` environment.
    fn tip_info(value: Option<&str>, description: Option<&str>) -> TokenInfo {
        TokenInfo {
            value: value.map(str::to_string),
            secret: false,
            source: TokenSource::Env("qa".into()),
            descriptions: description.into_iter().map(str::to_string).collect(),
        }
    }

    /// Short of room, stacked descriptions each keep a row: the selector's
    /// gives way before the option's, so the more specific one is never
    /// cut away behind the other's ellipsis.
    #[test]
    fn a_cut_stack_keeps_a_row_of_each_description() {
        let selector = "acting user for every request in this space, linked to a customer \
                        record in the billing system";
        let info = TokenInfo {
            descriptions: vec![selector.into(), "the premium one".into()],
            ..tip_info(Some("v"), None)
        };
        // 8 rows below the token: borders, the name row and two gaps take
        // 5, the value 1, leaving 2 for the descriptions.
        let rows = tip_rows(120, 9, &info);
        assert_eq!(rows.len(), 6, "{rows:?}");
        assert!(rows[4].starts_with("acting user"), "{rows:?}");
        assert!(rows[4].ends_with('\u{2026}'), "the selector's is cut: {rows:?}");
        assert_eq!(rows[5], "the premium one", "the option's survives: {rows:?}");
    }

    /// A selector field's two descriptions stack, each wrapped on its own:
    /// the selector's, then the option's, with no row between (#19).
    #[test]
    fn a_tooltips_descriptions_stack_each_wrapped_on_its_own() {
        let option = "the premium one, with an active subscription and a linked customer";
        let info = TokenInfo {
            descriptions: vec!["acting user".into(), option.into()],
            ..tip_info(Some("1001"), None)
        };
        let rows = tip_rows(120, 40, &info);
        assert_eq!(rows[4], "acting user", "{rows:?}");
        assert_eq!(rows[5..].join(" "), option, "{rows:?}");
    }

    /// Draws the `x` tooltip alone on a `w`×`h` screen, anchored at the
    /// top-left, and returns its text rows between the panel's borders
    /// (padding columns dropped, pill icons dropped, trailing spaces
    /// trimmed), top row first.
    fn tip_rows(w: u16, h: u16, info: &TokenInfo) -> Vec<String> {
        draw_tip(w, h, 0, info).expect("a tip is drawn").1
    }

    /// Draws the `x` tooltip alone on a `w`×`h` screen for a token on row
    /// `anchor_y`: its panel and [`tip_rows`]'s rows, or `None` when no
    /// tip fits.
    fn draw_tip(w: u16, h: u16, anchor_y: u16, info: &TokenInfo) -> Option<(Rect, Vec<String>)> {
        draw_tip_revealed(w, h, anchor_y, info, false)
    }

    /// [`draw_tip`], with a secret shown in clear when `revealed`.
    fn draw_tip_revealed(
        w: u16,
        h: u16,
        anchor_y: u16,
        info: &TokenInfo,
        revealed: bool,
    ) -> Option<(Rect, Vec<String>)> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = crate::hit::HitMap::default();
        let tip = crate::app::TokenTip { name: "x".into(), anchor: Rect::new(0, anchor_y, 5, 1) };
        terminal
            .draw(|f| {
                draw_var_tooltip(
                    f,
                    f.area(),
                    &crate::theme::Theme::dark(),
                    &tip,
                    info,
                    revealed,
                    None,
                    &mut hits,
                )
            })
            .unwrap();
        let panel = hits.rect_of(&crate::hit::Hit::TipPanel("x".into()))?;
        let buf = terminal.backend().buffer();
        // The pills' icons (Private Use Area glyphs) are dropped, so the
        // top row reads as its text alone.
        let rows = (panel.y + 1..panel.bottom() - 1)
            .map(|y| {
                (panel.x + 2..panel.right() - 2)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .chars()
                    .filter(|c| !('\u{F0000}'..='\u{FFFFD}').contains(c))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        Some((panel, rows))
    }

    /// #11: a long description word-wraps at the tooltip's max width
    /// instead of being cut short — every word is there, in order.
    #[test]
    fn a_long_tooltip_description_word_wraps_at_the_max_width() {
        let desc = "Bearer token for the QA tenant, issued by the admin console and \
                    rotated every Monday morning by the platform team";
        let rows = tip_rows(120, 40, &tip_info(Some("v"), Some(desc)));
        let wrapped = &rows[4..];
        assert!(wrapped.len() > 1, "{rows:?}");
        for row in wrapped {
            assert!(row.width() <= TOOLTIP_MAX_TEXT_W, "{row:?}");
            assert!(!row.contains('\u{2026}'), "nothing is cut: {row:?}");
        }
        assert_eq!(wrapped.join(" "), desc, "breaks fall between words");
    }

    /// Value only: the value, a blank row, the name. No description, no
    /// second blank row.
    #[test]
    fn a_tooltip_without_a_description_ends_at_its_name() {
        let rows = tip_rows(120, 40, &tip_info(Some("v"), None));
        assert_eq!(rows, ["v", "", "x  qa"]);
    }

    /// Description only (no value): the name row on top, a blank row, the
    /// description.
    #[test]
    fn a_tooltip_without_a_value_tops_with_its_name() {
        let rows = tip_rows(120, 40, &tip_info(None, Some("about x")));
        assert_eq!(rows, ["x  qa", "", "about x"]);
    }

    /// On a terminal narrower than the max width, value and description
    /// wrap to the room there is rather than being ellipsized at the edge
    /// — the value a pill strip narrower, so it never runs under them.
    #[test]
    fn a_tooltip_wraps_to_a_narrow_screen() {
        let value = "https://qa.example.com/a/rather/long/path";
        let desc = "the root every request in the qa space starts from";
        let rows = tip_rows(30, 40, &tip_info(Some(value), Some(desc)));
        let gap = rows.iter().position(String::is_empty).unwrap();
        assert_eq!(rows[..gap].concat(), value, "{rows:?}");
        assert_eq!(rows[gap + 1], "x  qa");
        assert_eq!(rows[gap + 3..].join(" "), desc, "{rows:?}");
        for row in &rows[..gap] {
            // 25 cells inside the 30-wide screen, less a gap and two pills.
            assert!(row.width() <= 18, "clear of the pills: {row:?}");
        }
        for row in &rows {
            assert!(row.width() <= 25, "inside the 30-wide screen: {row:?}");
            assert!(!row.contains('\u{2026}'), "{row:?}");
        }
    }

    /// With too little room for the whole tip, the value gives up rows
    /// first (down to one), then the description; each cut row says so.
    /// The room is what lies below the token (row 0 here), not the screen.
    #[test]
    fn a_short_screen_cuts_the_value_before_the_description() {
        let value = "a".repeat(3 * TOOLTIP_MAX_TEXT_W);
        let desc = "one two three four five six seven eight nine ten eleven twelve \
                    thirteen fourteen fifteen sixteen";
        let info = tip_info(Some(&value), Some(desc));
        // 8 rows below the token; borders, the name row and two gaps take
        // 5, leaving 3: the description's two, and one for the value.
        let rows = tip_rows(120, 9, &info);
        assert_eq!(rows.len(), 6, "{rows:?}");
        assert!(rows[0].ends_with('\u{2026}'), "the value is cut: {rows:?}");
        assert_eq!(rows[2], "x  qa");
        assert_eq!(rows[4..].join(" "), desc, "the description is whole: {rows:?}");
        // Two rows left: the value keeps one, the description the other.
        let rows = tip_rows(120, 8, &info);
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert!(rows[0].ends_with('\u{2026}'), "{rows:?}");
        assert!(rows[4].ends_with('\u{2026}'), "the description is cut too: {rows:?}");
        // One row left: each section keeps at least one row, so no tip
        // fits — never a tip that silently drops its description.
        assert_eq!(draw_tip(120, 7, 0, &info), None);
    }

    /// A tip too tall for either side of its token is cut to the roomier
    /// side and placed there — never clamped over the token it describes.
    #[test]
    fn a_tall_tip_is_cut_to_one_side_of_its_token_and_never_covers_it() {
        let info = tip_info(Some(&"a".repeat(30 * TOOLTIP_MAX_TEXT_W)), Some("about x"));
        // Token on row 10 of 20: 10 rows above it, 9 below.
        let (panel, rows) = draw_tip(120, 20, 10, &info).expect("a tip is drawn");
        assert_eq!(
            (panel.y, panel.bottom()),
            (0, 10),
            "above, flush against the token so the pointer can cross to it: {panel:?}"
        );
        // 10 rows above: borders, the name row and two gaps take 5, the
        // description 1, the value keeps the other 4 — cut. Its rows are
        // the max width less a gap and two pills.
        let value_w = TOOLTIP_MAX_TEXT_W - 1 - 6;
        assert_eq!(rows, {
            let mut v = vec!["a".repeat(value_w); 3];
            v.push("a".repeat(value_w - 1) + "\u{2026}");
            v.extend(["", "x  qa", "", "about x"].map(String::from));
            v
        });
        // Token on row 9: 9 above, 10 below — the tip goes below.
        let (panel, _) = draw_tip(120, 20, 9, &info).expect("a tip is drawn");
        assert_eq!((panel.y, panel.bottom()), (10, 20), "below the token: {panel:?}");
        // Mid-screen on a 9-row terminal: 4 rows each side, short of the
        // 7 a value and a description need — no tip, rather than one over
        // its token (the user's ruling).
        assert_eq!(draw_tip(120, 9, 4, &info), None);
    }

    /// Reveal / hide moves nothing, even for a secret whose value is
    /// empty: its mask shows one dot, and the clear value keeps that cell.
    #[test]
    fn revealing_an_empty_secret_keeps_the_tips_footprint() {
        let info = TokenInfo {
            value: Some(String::new()),
            secret: true,
            source: TokenSource::Env("a".into()),
            descriptions: Vec::new(),
        };
        let (masked, _) = draw_tip_revealed(120, 40, 0, &info, false).unwrap();
        let (clear, _) = draw_tip_revealed(120, 40, 0, &info, true).unwrap();
        assert_eq!(masked, clear);
    }

    /// A description of nothing but whitespace is no description: no
    /// blank section, and surrounding blank lines are not drawn either.
    #[test]
    fn a_blank_tooltip_description_draws_no_section() {
        let bare = tip_rows(120, 40, &tip_info(Some("v"), None));
        assert_eq!(tip_rows(120, 40, &tip_info(Some("v"), Some(" \n "))), bare);
        let rows = tip_rows(120, 40, &tip_info(Some("v"), Some("\nabout x\n")));
        assert_eq!(rows[3..], ["", "about x"], "{rows:?}");
    }

    /// A description is prose: its newlines fold like any other space, so
    /// a blank line inside it can't pass for the gap before a section.
    #[test]
    fn a_tooltip_description_folds_its_newlines() {
        let rows = tip_rows(120, 40, &tip_info(Some("v"), Some("Bearer token\n\nRotated weekly")));
        assert_eq!(rows[3..], ["", "Bearer token Rotated weekly"], "{rows:?}");
    }

    /// The final review's D-1 (plan M4b Deviation 20): the header's
    /// keycaps are spelled on the screen under any modal, so vim's save
    /// reads `:w` with a prompt, a confirm or a picker up (where `:` does
    /// not apply and vim binds no ctrl+s), never a blank keycap.
    #[test]
    fn the_header_keycaps_are_spelled_on_the_screen_under_any_modal() {
        use crate::action::Action;
        use crate::components::modal::Modal;
        use crate::config::KeyProfile;
        let alt_d = format!("{}+d", crate::keys::alt_label());
        let mut app = App::new_for_test();
        app.use_profile(KeyProfile::Vim);
        assert_eq!(header_keys(&app), (":w".to_string(), alt_d.clone()));
        app.update(Action::PromptNewRequest);
        assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
        assert_eq!(header_keys(&app), (":w".to_string(), alt_d.clone()), "under a prompt");
        app.push_modal(Modal::Confirm { title: "t".into(), body: "b".into(), choices: Vec::new() });
        assert_eq!(app.surface(), crate::keyroute::Surface::Dialog);
        assert_eq!(header_keys(&app), (":w".to_string(), alt_d.clone()), "under a confirm");
        app.update(Action::OpenPalette);
        assert_eq!(app.surface(), crate::keyroute::Surface::Picker);
        assert_eq!(header_keys(&app), (":w".to_string(), alt_d.clone()), "under a picker");
        app.use_profile(KeyProfile::Arrows);
        assert_eq!(header_keys(&app), ("^S".to_string(), alt_d), "arrows");
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

    /// The Settings tab paints both sections and its own footer chips —
    /// never the previously open tab's, whose keys do nothing here.
    #[test]
    fn the_settings_tab_paints_its_sections_and_its_own_chips() {
        let mut app = App::new_for_test();
        app.screen = Screen::Manage;
        app.manage.tab = crate::components::manage::ManageTab::Settings;
        let content = render(&mut app);
        for label in [
            "Settings",
            "Animations",
            "Hover hints",
            "jq Tab behavior",
            "AI command",
            "Clipboard command",
            "OSC 52 limit",
            "Files",
            "Edit",
            "Reset",
        ] {
            assert!(content.contains(label), "missing {label:?}");
        }
        assert!(content.contains("move"), "the row chips are advertised");
        assert!(!content.contains("rename"), "the list tabs' chips must not leak in: {content}");
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
            assert_eq!(cell.symbol(), " ", "no `│` glyph at the gutter column: {cell:?}");
            assert_eq!(cell.bg, app.theme.page, "gutter cell is a flat page fill: {cell:?}");
        }
    }
}
