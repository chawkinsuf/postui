//! Mouse routing: raw event handling, hover tracking, click dispatch
//! (`on_hit`), and scrollbar-thumb drags. Split from `app.rs`; every
//! method here is `impl App` and shares its state directly.

use super::*;
use crate::components::manage::ManageTab;
use crate::keyroute::{FieldCaret, FieldId};

/// A left press's click counts. `list` is a pair (same hit within 400 ms;
/// a third press is a single, so a fast triple click never toggles a
/// folder twice), which rows and buttons read in both profiles. `text`
/// counts up to four on a field's text in both profiles (spec 2026-10-14,
/// align the text mouse, A7; Vim's `'mousetime'` rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Clicks {
    pub(super) list: u8,
    pub(super) text: u8,
}

/// The window for a list click count: a pair on the same control.
const LIST_WINDOW_MS: u64 = 400;

/// A7's window for a click count on text, which also needs the same cell.
/// On trial (the user, 2026-10-14: "don't make it too hard to go back"):
/// arrows' text counted within [`LIST_WINDOW_MS`] on the same control
/// anywhere. To go back for arrows, give [`count_clicks`] the profile, and
/// there use that window and drop the cell check in the `text` arm.
const TEXT_WINDOW_MS: u64 = 500;

/// The last left press: what it hit, its cell, when, and its counts.
#[derive(Debug, Clone)]
pub(super) struct LastClick {
    hit: Hit,
    cell: (u16, u16),
    at: std::time::Instant,
    pub(super) clicks: Clicks,
}

/// The record a left press on `hit` at `cell` leaves, with its counts.
pub(super) fn count_clicks(
    last: Option<&LastClick>,
    hit: &Hit,
    cell: (u16, u16),
    at: std::time::Instant,
) -> LastClick {
    use std::time::Duration;
    let within =
        |ms| last.is_some_and(|l| at.saturating_duration_since(l.at) < Duration::from_millis(ms));
    let list = match last {
        Some(l) if l.hit == *hit && l.clicks.list == 1 && within(LIST_WINDOW_MS) => 2,
        _ => 1,
    };
    let text = match last {
        // The same control too: a press can open a field under the cell.
        Some(l)
            if l.hit == *hit && l.cell == cell && l.clicks.text < 4 && within(TEXT_WINDOW_MS) =>
        {
            l.clicks.text + 1
        }
        _ => 1,
    };
    LastClick { hit: hit.clone(), cell, at, clicks: Clicks { list, text } }
}

/// The footer's quit chip, whichever quit the profile and surface spell it
/// as (`spell::quit_action`). It is the app's on every screen, so the
/// testbed lets only it through.
fn is_quit_chip(hit: &Hit) -> bool {
    matches!(
        hit,
        Hit::FooterChip(
            Action::Quit | Action::GatedQuit | Action::CloseWindow { write: false, force: false }
        )
    )
}

/// A copy pill's alt+click copies the whole header, `Name: value`, not
/// its value (spec 2026-10-26 H6). Alt, not shift (#159): terminals keep
/// shift+click for their own text selection under mouse capture, Ghostty
/// included, so a shift+click never reached the pill; alt+click does.
fn alt_click(m: &ratatui::crossterm::event::MouseEvent) -> bool {
    m.modifiers.contains(ratatui::crossterm::event::KeyModifiers::ALT)
}

/// The hit a text field is drawn under: the one map between the two.
fn field_hit(field: FieldId) -> Option<Hit> {
    Some(match field {
        FieldId::Url => Hit::UrlBar,
        FieldId::Body => Hit::BodyEditor,
        FieldId::Jq => Hit::ResponseJqBar,
        FieldId::TableCell { row, col } => Hit::TableCell { row, col: u8::try_from(col).ok()? },
        FieldId::VmForm(field) => Hit::VmFormField(field),
        FieldId::VmGrid { row, col } => Hit::VmEntryCell { row, col },
        FieldId::Settings(field) => Hit::SettingsControl(field),
        FieldId::ModalInput(i) => Hit::ModalInput(i),
        _ => return None,
    })
}

impl App {
    /// Routes a raw terminal mouse event against `self.hits`, the `HitMap`
    /// `ui::draw` rebuilt on the last frame. No layout is needed any more:
    /// every clickable region — pane background, button, chip — was
    /// registered there already, topmost-wins.
    ///
    /// - `Moved` resolves the hit under the pointer and, if it differs from
    ///   `self.hovered`, stores it and asks for a redraw (so hover styling
    ///   updates); the same hit twice in a row is a no-op.
    /// - `Down(Left)` resolves the hit, tracks single vs. double click (same
    ///   hit within 400ms), and dispatches through `on_hit`.
    /// - `Up(Left)` clears any in-progress drag.
    /// - Wheel events scroll the body editor when over it, else scroll the
    ///   pane under the pointer. While a modal is open, wheel is a no-op
    ///   here — modal-list scrolling is a later task.
    pub fn handle_mouse(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::MouseEventKind;
        // Where the keyboard is before this event, as `handle_key` learns
        // it: a click's focus change counts for `<C-w>l` and `p` (plan M4a
        // Deviation 6).
        self.note_pane();
        // A press drops a half-typed key (plan M1 Deviation 15): a `g`
        // never survives a click. It drops an armed quit too, so the two
        // ctrl+c presses are back to back (task #85).
        let mut disarmed = false;
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.router.clear();
            disarmed = self.disarm_quit().is_some();
        }
        let edge = self.vim_mouse_start(&m);
        let changed = self.handle_mouse_inner(m);
        self.vim_mouse_end(edge);
        // The op safety net at the event boundary, as in `handle_key`: a
        // drag release commits without an action and can bail between
        // `begin_op` and its record.
        self.op_in_flight = false;
        let ended = self.vim_event_end();
        let noted = self.toast_response_note();
        self.arm_pending_toasts();
        // Same reason as `handle_key`: a click that only pops a modal
        // never reaches `update`, and an unanswered startup gate must
        // not be left down.
        let regated = self.reraise_startup_config_gate();
        let asked = self.raise_keymap_choice();
        changed || ended || noted || regated || asked || disarmed
    }

    fn handle_mouse_inner(&mut self, m: ratatui::crossterm::event::MouseEvent) -> bool {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        // Every event carries the pointer's position, motion reports or
        // not: a press moves the pointer just as truly as a `Moved` does.
        // Recording it here keeps the `{{token}}` tooltip (which is drawn
        // from `pointer`) honest in terminals that report no motion, where
        // a tip opened by one hover would otherwise hang over the UI
        // through every later click — including the clicks it covers.
        self.pointer = Some((m.column, m.row));
        // The control under the pointer, `{{token}}` overlays aside (they
        // have their own hover track, `hovered_token`). Resolved once here
        // for both the tooltip hold and the motion arm's hover styling.
        let under = self.hits.hit_at_ignoring_var_tokens(m.column, m.row).cloned();
        // The tooltip hold: the pointer on the last-drawn tip's panel or a
        // pill holds that tip, so it survives a click on its pills and
        // drops on the first event landing elsewhere. (The tip hits in
        // the map all belong to `drawn_tip` — one tip per frame.)
        self.tip_held = under.as_ref().and_then(Hit::tip_name).is_some();

        match m.kind {
            // Terminals report pointer motion with a button held as `Drag`,
            // not `Moved`, so a thumb drag arrives as either depending on
            // whether the terminal tracks button state; both drive the drag.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                // A live sidebar row drag owns every motion event until the
                // button comes up, whichever pane the pointer wanders over.
                if self.sidebar.drag.is_some() {
                    return self.sidebar_drag_to(m.column, m.row);
                }
                // So does a live pane resize: the divider follows the
                // pointer wherever it goes, and hover stays frozen on the
                // handle (it stays lit) until the release. It follows
                // `Moved` too, for the reason this arm's header gives:
                // terminals that don't track button state report a held
                // drag as `Moved`. Once this drag has had a `Drag(Left)`,
                // though, the terminal does track it, so a bare `Moved`
                // means the button is up and the release was lost (review
                // round 7 M5): the resize ends there, as a release would,
                // and the `Moved` is a hover. Any other lost release is
                // caught by the next press or a focus loss. Every end keeps
                // the size where it got to and saves it (spec P2a).
                // A key can also take the Main screen away mid-drag (`alt+r`
                // to Manage, a dialog such as the palette): the resize ends
                // at the next motion too, which then sizes nothing it can't
                // show (review round 8 N4).
                if let Some(drag) = &mut self.resize {
                    let lost = m.kind == MouseEventKind::Moved && drag.tracked;
                    drag.tracked |= m.kind != MouseEventKind::Moved;
                    if lost || self.screen != Screen::Main || !self.modals.is_empty() {
                        self.finish_resize();
                        self.resync_hover();
                        return true;
                    }
                    return self.resize_to(m.column, m.row) && self.update(Action::Render);
                }
                // A press armed on a row becomes a drag the moment the
                // pointer reaches a *different* row. The `hit_at` check
                // keeps a press-then-wander over the header (or any other
                // chrome) from starting one; once started, motion anywhere
                // is mapped by `row_at_y`.
                // The row index recorded at press time can be stale by now
                // (a `ReloadProjectFiles` on focus-gained, a folder toggle
                // or a background refresh can have rebuilt the rows), so
                // the drag target is re-resolved by the pressed slug; a
                // press whose request is gone promotes nothing. A modal
                // opened by the press itself (a broken row's error) owns
                // the pointer, so nothing promotes under one either.
                // A live thumb drag or text sweep owns the button as well
                // (a live pane resize returned above): a press left armed
                // by a lost release never promotes over one of those.
                let other_drag = self.drag.is_some() || self.text_drag.is_some();
                if let Some((_, pressed)) = self.sidebar_press.clone()
                    && self.modals.is_empty()
                    && !other_drag
                    && matches!(self.hits.hit_at(m.column, m.row), Some(Hit::SidebarRow(_)))
                {
                    match self
                        .sidebar
                        .rows
                        .iter()
                        .position(|r| matches!(r, Row::Request { slug: s, .. } if *s == pressed))
                    {
                        None => self.sidebar_press = None,
                        Some(i) if self.sidebar.row_at_y(m.row) != i => {
                            let space = self.active_space();
                            if self.sidebar.begin_drag(i, &space) {
                                self.hovered = None;
                                return self.sidebar_drag_to(m.column, m.row)
                                    | self.update(Action::Render);
                            }
                        }
                        Some(_) => {}
                    }
                }
                // The Manage screen's Spaces and Environments lists run
                // the same two-step: a live drag owns every motion event,
                // and an armed press promotes the moment the pointer
                // reaches another row. The pressed row index can be stale
                // (a reload relisted the tab), so the drag target is
                // re-resolved by name; a press whose item is gone promotes
                // nothing.
                if self.manage.list.drag.is_some() {
                    return self.manage_drag_to(m.column, m.row);
                }
                if let Some((_, pressed)) = self.manage_press.clone()
                    && self.modals.is_empty()
                    && !other_drag
                    && matches!(self.hits.hit_at(m.column, m.row), Some(Hit::ManageRow(_)))
                {
                    let tab = self.manage.tab;
                    match self.manage_items(tab).iter().position(|n| *n == pressed) {
                        None => self.manage_press = None,
                        Some(i) if self.manage.list.row_at_y(m.row) != i => {
                            let Self { project, manage, .. } = self;
                            let began =
                                project.as_ref().is_some_and(|p| manage.list.begin_drag(i, tab, p));
                            if began {
                                self.hovered = None;
                                return self.manage_drag_to(m.column, m.row)
                                    | self.update(Action::Render);
                            }
                        }
                        Some(_) => {}
                    }
                }
                if let Some(drag) = self.drag.as_ref() {
                    return if drag.horizontal {
                        self.drag_to_h(m.column)
                    } else {
                        self.drag_to(m.row)
                    };
                }
                if m.kind != MouseEventKind::Moved && self.text_drag.is_some() {
                    // Button-held motion with a sweep armed by the press
                    // that started it: a text-selection drag.
                    return match self.text_drag {
                        Some(TextDrag::Response) => {
                            self.session.response.drag_selection_to(m.column, m.row)
                        }
                        Some(TextDrag::Field(field)) => self.drag_text(field, m.column, m.row),
                        None => unreachable!(),
                    };
                }
                // Button-held motion with no sweep armed and no sidebar or
                // scrollbar drag live (both checked above) carries no drag
                // meaning: fall through to hover tracking. This also heals
                // a terminal that lost the button release — Ghostty on
                // Linux, after a right click cancels a sidebar drag, keeps
                // sending `Drag(Left)` motion (button already released)
                // until the next real `Down(Left)`/`Up(Left)` pair; without
                // this, hover styling freezes until that next click.
                // Two independent hover tracks: the control under the
                // pointer (styling), and any `{{token}}` drawn on top of it
                // (the value tooltip). A token must not steal its control's
                // hover styling, and leaving one must drop the tooltip on
                // the very next motion event.
                let hit = under;
                let token =
                    self.hits.var_token_at(m.column, m.row).map(|(name, _)| name.to_string());
                // The pointer's exact position (recorded above) is not part
                // of "changed": the tooltip is anchored at the token's own
                // drawn rect, so moving within one token changes nothing.
                let hit_changed = hit != self.hovered;
                let changed = hit_changed || token != self.hovered_token;
                self.hovered = hit;
                self.hovered_token = token;
                if hit_changed {
                    self.begin_hover_fade();
                }
                changed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // A real left press cannot arrive while a sidebar drag is
                // still live — the button that started it would have to
                // still be down. If one is live here, the terminal lost
                // the release (Ghostty on Linux, verified with
                // `--keydump`: after a right click cancels a drag it keeps
                // sending `Drag(Left)` motion, then eventually a fresh
                // `Down(Left)`/`Up(Left)` pair with no release in between).
                // Cancel the stale drag first, then handle this press
                // normally. The hit map still describes the frame that
                // painted the drag's *preview* order, and cancelling snaps
                // the rows back without a redraw, so a row hit is resolved
                // through what was painted under the pointer — the row the
                // user clicked — and re-found in the restored rows.
                // Every other drag kind the same lost release can leave
                // live is cleared too: the scrollbar thumb would otherwise
                // keep tracking bare pointer motion, and a stale press
                // (its release lost the same way) would promote a row drag
                // out from under whatever this press starts. `on_hit`
                // re-arms the press when this click lands on a row.
                // `{{token}}` overlays are skipped: a click on one is a
                // click on the text it sits in (caret placement, cell
                // edit), never a dialog — the tooltip's edit pill is the
                // route to the variable itself.
                let hit = self.hits.hit_at_ignoring_var_tokens(m.column, m.row).cloned();
                let hit = self.cancel_stale_drags(hit);
                // A click anywhere but the jq bar itself is Enter for a
                // focused bar: the filter is already applied (every edit
                // re-ran it), so all that is left to do is leave the bar.
                // Modal and scrollbar hits are exempt, like every other
                // click-away rule: a popup over the pane or a scroll must
                // not pull the caret out from under what was being typed.
                // Nor is the bar's own switch (the header 󰈲, the footer's
                // `unfilter`) a click away: an empty bar hides once it
                // blurs, and the toggle would then read it as closed and
                // open it again. `ToggleJqBar` blurs as it closes. Its
                // switch-off is the bar's own state, part of the bar's close
                // as Esc's is, so the filter typed and the switch are one
                // step, by click as by key (task #64 records only a close
                // that comes with an edit elsewhere).
                // Two rules cover the rest, rather than a list of controls.
                // Anything on the bar's own row (its text, the chip, the ↺,
                // the AI button) is the bar's: what it does, it does with
                // the caret kept, as its key does. And so is every footer
                // chip: while the bar is focused, the footer is the bar's
                // own chip set (its `views`, `revert`, `unfilter`,
                // `describe…`), and its `commands` and `quit` keep the
                // caret as their keys do.
                // A press on a window separator is exempt as a scrollbar is:
                // it disturbs nothing ([`Self::is_separator_press`]).
                let on_bar_row = self.hits.rect_of(&Hit::ResponseJqBar).is_some_and(|bar| {
                    bar.y == m.row && self.hits.pane_at(m.column, m.row) == Some(PaneId::Response)
                });
                let jq_blurred = self.session.response.jq_focused()
                    && self.modals.is_empty()
                    && !on_bar_row
                    && !hit.as_ref().is_some_and(|h| self.is_separator_press(h))
                    && !matches!(
                        hit,
                        Some(
                            Hit::ResponseJqButton
                                | Hit::FooterChip(_)
                                | Hit::ScrollbarThumb(_)
                                | Hit::ScrollbarTrack(..)
                                | Hit::HScrollThumb(_)
                                | Hit::HScrollTrack(..)
                        )
                    );
                if jq_blurred {
                    self.leave_text_field();
                    self.session.response.set_jq_focus(false);
                }
                let Some(hit) = hit else {
                    // Bare background is still a click *away* from an
                    // in-place edit. The Variable Manager has plenty of it
                    // — under the last grid row, beside the title buttons —
                    // and landing there must save what was typed, the same
                    // as landing on a control (`on_hit`'s commit-first
                    // rule). Without this the typed text just hangs there
                    // with no way to tell it isn't saved.
                    //
                    // The Settings tab has the same bare background --
                    // below the last row, right of the label column --
                    // and the same rule: the edit commits, and the
                    // keyboard cursor goes with it, so no control is
                    // left lifted on a tab the pointer has walked off.
                    //
                    // With a dialog open the point is the dialog's (a gap
                    // between its footer chips, say), and the Settings
                    // field or Manager input under it stays open
                    // (`on_hit`'s dialog rule, tasks #73 and #75).
                    let manager_live = self.modals.is_empty()
                        && (self.varmanager.form.editing.is_some()
                            || self.varmanager.grid.editing.is_some());
                    let settings_live = self.modals.is_empty()
                        && (self.settings.editing.is_some() || self.settings.focused);
                    if !manager_live && !settings_live {
                        return jq_blurred;
                    }
                    if manager_live && !self.close_manager_input() {
                        return self.update(Action::Render);
                    }
                    if settings_live
                        && (self.settings.editing.is_none()
                            || self.commit_settings_edit_for_click())
                    {
                        self.settings.focused = false;
                    }
                    return self.update(Action::Render);
                };
                // The testbed is a dead end for the mouse exactly like it is
                // for the keyboard (see `App::handle_key`'s `Screen::Testbed`
                // branch): every click is inert except the footer's quit
                // chip. Without this, the header/footer chrome — drawn and
                // hit-registered unconditionally on every screen — would let
                // a click open the palette, a project/env chooser, or the
                // Variable Manager from underneath the showcase, with no way
                // back short of quitting.
                if self.screen == Screen::Testbed && !is_quit_chip(&hit) {
                    return false;
                }
                // Both counts at once (`count_clicks`): rows and buttons read
                // the list count, which pairs, so a fast triple click never
                // toggles a folder twice; text reads the text count.
                let press = count_clicks(
                    self.last_click.as_ref(),
                    &hit,
                    (m.column, m.row),
                    std::time::Instant::now(),
                );
                let clicks = press.clicks;
                self.last_click = Some(press);
                let was_content =
                    self.editor.sub_focus == crate::components::editor::SubFocus::Content;
                let changed = self.on_hit(hit, clicks, m) | jq_blurred;
                if !was_content
                    && self.editor.sub_focus == crate::components::editor::SubFocus::Content
                {
                    self.begin_focus_fade();
                }
                changed
            }
            // A right click targets the row under the pointer: it moves the
            // selection there first (so the menu's flows, which all read the
            // selection, act on what was clicked), then opens whatever menu
            // that hit offers. Hits with no menu still get the selection
            // move — right-clicking a row selects it, menu or not. For
            // sidebar rows the move is provisional: dismissing the menu
            // without choosing anything restores the previous selection
            // (see `App::sidebar_menu_revert`).
            MouseEventKind::Down(MouseButton::Right) => {
                // A right click during a live row drag cancels it, exactly
                // like a release outside the sidebar or Escape: no context
                // menu, no selection change, nothing written. This exists
                // because Ghostty (on Linux, verified with `--keydump` and
                // mouse reporting off) delivers no key event for the first
                // Escape pressed while a mouse button is held — the second
                // arrives; the cause is unconfirmed and is Escape-specific.
                // The app cannot recover a key it never receives, so a
                // right click is the in-band cancel.
                // The same lost release then leaves every other drag kind
                // live too (a thumb drag, a text sweep, a pane resize, which
                // keeps the size it reached and saves it) and would leave an
                // armed press ready to promote into a drag out from under
                // the menu this click is about to open: all of it ends
                // here, exactly as a `Down(Left)` ends it.
                if self.sidebar.drag.is_some() {
                    return self.finish_sidebar_drag(false);
                }
                if self.manage.list.drag.is_some() {
                    return self.finish_manage_drag(false);
                }
                self.cancel_stale_drags(None);
                // Tokens are a left-click affordance only: a right click
                // belongs to the row/cell under them and its context menu.
                let Some(mut hit) = self.hits.hit_at_ignoring_overlays(m.column, m.row).cloned()
                else {
                    return false;
                };
                let mut changed = false;
                // An open context menu doesn't own the right button the way
                // it owns the left: a second right click re-targets. It
                // dismisses the menu through `Action::Close` (running the
                // same sidebar-selection revert any click-away does), then
                // acts on whatever sits under the pointer beneath the
                // overlay — opening that row's menu, or nothing if there's
                // nothing to menu. Only the dropdown gets this treatment;
                // dialog modals keep the right button inert.
                if hit == Hit::ModalOutside && matches!(self.modals.top(), Some(Modal::Dropdown(_)))
                {
                    changed |= self.update(Action::Close);
                    match self.hits.hit_at_under_modal(m.column, m.row).cloned() {
                        Some(under) => hit = under,
                        None => return changed,
                    }
                }
                // In vim a right press on the text of the field being
                // edited extends its selection (spec 2026-10-14 R9). It
                // sweeps nothing.
                if let FieldCaret::Live(field) = self.field_caret()
                    && field_hit(field).as_ref() == Some(&hit)
                    && let Some(at) = self.text_at(field, m.column, m.row, true, false)
                    && self.vim_text_extend(field, at)
                {
                    return true;
                }
                // A text surface — the URL bar, the body, the response
                // text, or the one cell/field currently under edit — gets
                // the Copy/Paste menu instead of a row menu, and skips the
                // click-away commits below (which would end the edit whose
                // selection Copy reads). Any other part of a row falls
                // through to the row menu exactly as before.
                if let Some(items) = self.text_surface_menu(&hit) {
                    return self.open_context_menu(m.column, m.row, items) || changed;
                }
                // A right click is a click away from whatever detail-pane
                // cell was being typed into, exactly like a left click
                // (see `on_hit`'s blanket rule): commit it *before*
                // anything below reads or reshapes the rows underneath.
                // Skipping this let a menu action (Delete…, Rename…) shift
                // the option rows out from under a still-live `GridEdit`,
                // whose row index would then address a different record —
                // and the next click-away would write the typed text into
                // that wrong option. A close the input refuses (spec §5: the
                // typed text survives) swallows the press: no menu is
                // offered over the typing. A right press in a dialog is the
                // dialog's, as a left one is (`on_hit`'s dialog rule).
                if self.modals.is_empty() && !self.close_manager_input() {
                    return self.update(Action::Render);
                }
                // A table hit is normalized to `TableRow(resolved)` for the
                // menu lookup below: right-clicking any part of the row (a
                // cell, the checkbox, the 󰅖) opens the same row menu, and
                // `resolve_table_row_across_commit` re-numbers `i` past
                // whatever commit just landed — see its own doc comment.
                let mut menu_hit = hit.clone();
                // For sidebar rows: the selection to restore if the menu
                // opens and is then dismissed without choosing anything
                // (see `App::sidebar_menu_revert`).
                let mut menu_revert = None;
                match &hit {
                    Hit::SidebarRow(i) | Hit::SidebarFolderArrow(i) => {
                        changed |= self.update(Action::FocusPane(PaneId::Sidebar));
                        changed |= self.sidebar.selected != Some(*i);
                        menu_revert = Some(self.sidebar.selected);
                        self.set_sidebar_selected(*i);
                    }
                    Hit::VmLeftRow(i) => {
                        changed |= self.varmanager.left_cursor != *i;
                        self.varmanager.select_row(*i);
                    }
                    // An Environments/Spaces row: the click lands the
                    // list's cursor on it (the detail pane follows) before
                    // its menu opens, like a left click would.
                    Hit::ManageRow(i) => {
                        changed |= self.manage.list.cursor != *i;
                        self.manage.list.cursor = *i;
                    }
                    // Same rule for a grid row: the click moves the grid's
                    // cursor onto it (and the keyboard with it) before its
                    // menu opens.
                    Hit::VmEntryCell { row, .. } | Hit::VmEntryRadio(row) => {
                        let col = match &hit {
                            Hit::VmEntryCell { col, .. } => *col,
                            _ => 0,
                        };
                        changed |= self.varmanager.grid.cursor != (*row, col)
                            || self.varmanager.focus != VmFocus::Grid;
                        self.varmanager.grid.cursor = (*row, col);
                        self.varmanager.focus = VmFocus::Grid;
                    }
                    Hit::TableRow(i)
                    | Hit::TableCheckbox(i)
                    | Hit::TableCopy(i)
                    | Hit::TableDelete(i)
                    | Hit::TableCell { row: i, .. } => {
                        let i = *i;
                        changed |= self.update(Action::FocusPane(PaneId::Editor));
                        self.move_editor_caret(SubFocus::Content);
                        match self.resolve_table_row_across_commit(i) {
                            Some(resolved) => {
                                let stop = Stop::of_row(resolved, self.editor.table_len());
                                changed |= self.editor.table.selected() != Some(stop);
                                self.editor.table.set_selected(Some(stop));
                                menu_hit = Hit::TableRow(resolved);
                            }
                            // The clicked row was a ghost row that just
                            // discarded itself on commit: nothing left to
                            // menu.
                            None => return self.update(Action::Render) || changed,
                        }
                    }
                    _ => {}
                }
                match self.context_menu_for(&menu_hit) {
                    Some(items) => {
                        let opened = self.open_context_menu(m.column, m.row, items);
                        if opened {
                            self.sidebar_menu_revert = menu_revert;
                        }
                        opened || changed
                    }
                    None => changed,
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // The press is disarmed by any release, drag or not, and
                // every drag kind ends: a finished text sweep keeps its
                // selection, only the sweep state ends. All of them are
                // taken before the row drags are settled so that none can
                // outlive the release, whichever kind ends it.
                self.sidebar_press = None;
                self.manage_press = None;
                let had_text = self.text_drag.take().is_some();
                let ended = self.drag.take().is_some() || had_text;
                // A resize ends where it got to, saved if it moved. The
                // strip's click already happened at the press (spec E2a),
                // so the release adds nothing to it. Hover was frozen on
                // the handle for the drag: it re-resolves under the
                // pointer now.
                if self.resize.is_some() {
                    self.finish_resize();
                    self.resync_hover();
                    return true;
                }
                // A row drag writes its new order only when it is dropped
                // on the sidebar; a release anywhere else cancels it.
                if self.sidebar.drag.is_some() {
                    let inside = self.sidebar_drag_inside(m.column, m.row);
                    return self.finish_sidebar_drag(inside) | ended;
                }
                // The Manage list has no `Hit::Pane` of its own, so the
                // drop test is the list rect the last draw recorded (the
                // rect containment `pane_at` would do), plus the row hits
                // themselves.
                if self.manage.list.drag.is_some() {
                    let inside = self.manage_drag_inside(m.column, m.row);
                    return self.finish_manage_drag(inside) | ended;
                }
                ended
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if !self.modals.is_empty() {
                    let d = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                    return self.modals.scroll_top(d);
                }
                if matches!(self.hits.hit_at(m.column, m.row), Some(Hit::BodyEditor))
                    && self.editor.handle_mouse(m)
                {
                    return self.update(Action::Render);
                }
                if self.screen == Screen::Manage {
                    let d = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                    if self.manage.tab == ManageTab::Variables {
                        self.varmanager.handle_scroll_at(m.column, m.row, d);
                    } else {
                        self.manage.list.handle_scroll(d);
                    }
                    return self.update(Action::Render);
                }
                if let Some(pane) = self.hits.pane_at(m.column, m.row) {
                    let d = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                    return self.update(Action::ScrollPane(pane, d));
                }
                false
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                // A sideways wheel notch (shift+wheel in most terminals)
                // moves the response viewport over its clipped columns.
                // Modals and the Variable Manager have nothing to scroll
                // sideways, and neither does any other pane.
                if !self.modals.is_empty() || self.screen == Screen::Manage {
                    return false;
                }
                if self.hits.pane_at(m.column, m.row) == Some(PaneId::Response) {
                    let d = if m.kind == MouseEventKind::ScrollLeft {
                        -crate::components::response::H_SCROLL_STEP
                    } else {
                        crate::components::response::H_SCROLL_STEP
                    };
                    self.session.response.handle_scroll_h(d);
                    return self.update(Action::Render);
                }
                false
            }
            _ => false,
        }
    }

    /// Ends every drag a lost button release can leave live — the two row
    /// drags, a scrollbar thumb drag, a text sweep, a pane resize — and
    /// disarms both row presses, because a fresh button press (or a focus
    /// loss) proves the button that started them is no longer held.
    /// Nothing is written but a resize's size, which is kept and saved.
    ///
    /// `hit` is the click being handled, resolved against the last frame.
    /// A live row drag painted that frame in its *preview* order and the
    /// cancel snaps the rows back without a redraw, so a row hit is
    /// mapped to the row that was painted there and re-found in the
    /// restored list; every other hit passes through unchanged. Returns
    /// the hit to dispatch.
    pub fn cancel_stale_drags(&mut self, hit: Option<Hit>) -> Option<Hit> {
        let mut hit = hit;
        if self.sidebar.drag.is_some() {
            let painted = match &hit {
                Some(Hit::SidebarRow(i)) => self.sidebar.rows.get(*i).cloned(),
                _ => None,
            };
            self.finish_sidebar_drag(false);
            if let Some(row) = painted {
                hit = self
                    .sidebar
                    .rows
                    .iter()
                    .position(|r| Sidebar::same_row(r, &row))
                    .map(Hit::SidebarRow);
            }
        }
        if self.manage.list.drag.is_some() {
            let tab = self.manage.tab;
            let painted = match (&hit, self.manage.list.drag.as_ref()) {
                (Some(Hit::ManageRow(i)), Some(drag)) => drag.working.get(*i).cloned(),
                _ => None,
            };
            self.finish_manage_drag(false);
            if let Some(name) = painted {
                hit = self.manage_items(tab).iter().position(|n| *n == name).map(Hit::ManageRow);
            }
        }
        self.sidebar_press = None;
        self.manage_press = None;
        self.drag = None;
        self.text_drag = None;
        // A resize is applied live, so it ends where it got to and is
        // saved, like a thumb drag and unlike a row reorder (spec P2a).
        self.finish_resize();
        hit
    }

    /// Terminal focus left the window. A button held at that moment is
    /// released elsewhere, so every drag ends here as a cancel; the rows
    /// snap back and the keyboard is the user's again.
    ///
    /// The hover highlight goes too. It is a statement about where the
    /// pointer is, and once the pointer has left the window that statement
    /// is false with nothing to correct it: the move handler only hit-tests
    /// on motion *over* the terminal, so a control lit on the way out stays
    /// lit indefinitely. `pointer` is forgotten alongside it, or
    /// [`Self::resync_hover`] would re-light the same control from the stale
    /// position after the very next frame.
    ///
    /// This is the only leaving the terminal reports *portably*. A pointer
    /// that crosses out while the window keeps focus (click-to-focus, a
    /// second monitor) sends nothing under the standard protocol: xterm's
    /// mouse modes (9, 1000-1006, 1015, 1016) have no leave event, and 1004
    /// reports focus, not the pointer. So that case is unhandled here.
    ///
    /// It is not, however, impossible — should it ever be worth doing:
    ///
    /// kitty extends SGR-Pixel mouse reporting to cover it. A leave arrives
    /// as an ordinary SGR pixel event with **bit 8 set on the first number**;
    /// every other bit and both coordinates are meaningless and must be
    /// ignored (`sw.kovidgoyal.net/kitty/misc-protocol/`). Three things make
    /// it a bigger job than it sounds:
    ///
    /// 1. It rides on mode `?1016h`, which switches *all* mouse reporting to
    ///    pixels. Every event would then need a pixel->cell conversion before
    ///    reaching the (cell-based) hit map, using the terminal's cell size
    ///    from `ws_xpixel`/`ws_ypixel` — which crossterm documents as
    ///    possibly unimplemented or zero. Getting that wrong breaks every
    ///    click in the app, not just this edge case.
    /// 2. crossterm enables 1000/1002/1003/1015/1006 and parses none of
    ///    1016, so the bytes would have to be read directly. There is
    ///    precedent for that in this codebase — `theme::osc` already reads
    ///    fd 0 behind crossterm's back for colour queries — but it is not
    ///    free.
    /// 3. Ghostty implements 1016 but its support for the *leave* extension
    ///    is unverified. Test it against the real terminal before building
    ///    anything on it, and keep a path for terminals without it.
    ///
    /// A weaker signal exists too: kitty and Ghostty report negative
    /// coordinates for motion outside the window while a button is held
    /// (xterm dropped those events in patch 404, and foot followed). That is
    /// drag-only, so it says nothing about a hover.
    ///
    /// Returns whether a repaint is needed.
    pub fn on_focus_lost(&mut self) -> bool {
        let live = self.sidebar.drag.is_some()
            || self.manage.list.drag.is_some()
            || self.drag.is_some()
            || self.text_drag.is_some()
            || self.resize.is_some();
        let lit = self.hovered.is_some() || self.hovered_token.is_some();
        self.cancel_stale_drags(None);
        self.hovered = None;
        self.hovered_token = None;
        self.pointer = None;
        live || lit
    }

    /// The window has focus back: what changed while postui was away
    /// shows — files edited outside it, and a curl another app copied
    /// (spec 2026-10-15 curl import visibility V4). Returns whether a
    /// repaint is needed.
    pub fn on_focus_gained(&mut self) -> bool {
        let clipboard = self.check_clipboard();
        self.update(Action::ReloadProjectFiles) | clipboard
    }

    /// Re-resolves `hovered`/`hovered_token` from the last known pointer
    /// position against the just-rebuilt frame's hit map. The move handler
    /// only hit-tests on actual motion, so without this a control that
    /// appears (or vanishes) *under* a stationary pointer — a hover-revealed
    /// row button, a pane's action pills after a view switch — would keep
    /// stale hover styling until the mouse next moved. Called after every
    /// frame draw; returns whether anything changed (the caller schedules a
    /// repaint). Inert mid-drag, matching the move handler.
    pub fn resync_hover(&mut self) -> bool {
        if self.drag.is_some()
            || self.text_drag.is_some()
            || self.resize.is_some()
            || self.sidebar.drag.is_some()
            || self.manage.list.drag.is_some()
        {
            return false;
        }
        let Some((x, y)) = self.pointer else {
            return false;
        };
        let hit = self.hits.hit_at_ignoring_var_tokens(x, y).cloned();
        let token = self.hits.var_token_at(x, y).map(|(name, _)| name.to_string());
        let hit_changed = hit != self.hovered;
        let changed = hit_changed || token != self.hovered_token;
        self.hovered = hit;
        self.hovered_token = token;
        if hit_changed {
            self.begin_hover_fade();
        }
        changed
    }

    /// Recomputes the pointer shape (task 8d) from `self.hovered` — the same
    /// hover state `ui::draw` already styles from, so this piggybacks on the
    /// existing hover-change path rather than running its own hit test.
    /// Returns the new shape only when it differs from the one last emitted,
    /// which the caller (main.rs's event loop) writes as a Kitty OSC 22
    /// escape after the frame draws; `None` means nothing to write this
    /// frame. Pure aside from updating `last_pointer_shape`, so it's
    /// testable without a terminal. A live sidebar row drag overrides the
    /// hover shape with the closed hand, since hover is frozen for the
    /// length of the drag; a live pane resize keeps its handle's resize
    /// shape wherever the pointer wanders (spec S3, E4).
    pub fn pointer_shape_update(&mut self) -> Option<PointerShape> {
        let shape = if self.sidebar.drag.is_some() || self.manage.list.drag.is_some() {
            PointerShape::Grabbing
        } else {
            match self.resize.map(|drag| drag.kind) {
                Some(ResizeHandle::Sidebar) => PointerShape::ColResize,
                Some(ResizeHandle::Split { .. }) => PointerShape::RowResize,
                None => PointerShape::for_hit(self.hovered.as_ref()),
            }
        };
        if shape == self.last_pointer_shape {
            return None;
        }
        self.last_pointer_shape = shape;
        Some(shape)
    }

    /// The scroll state `pane` would draw a scrollbar from right now — the
    /// same [`ScrollbarSpec`] its `draw` builds, so drag math and the drawn
    /// thumb can never disagree. `None` when the pane has nothing scrollable
    /// (or has not been drawn yet).
    pub fn scrollbar_spec(&self, pane: PaneId) -> Option<ScrollbarSpec> {
        // The Variable Manager screen replaces the whole body, sidebar
        // included: while it is up, the sidebar's pane slot belongs to its
        // left list (see `VarManager::scrollbar_spec`).
        if self.screen == Screen::Manage {
            // The other tabs' list draws no scrollbar of its own, so the
            // sidebar's pane slot has nothing to report there.
            if self.manage.tab != ManageTab::Variables {
                return None;
            }
            return self.varmanager.scrollbar_spec().filter(|s| s.pane == pane);
        }
        match pane {
            PaneId::Sidebar => self.sidebar.scrollbar_spec(),
            PaneId::Editor => self.editor.scrollbar_spec(),
            PaneId::Response => self.session.response.scrollbar_spec(),
        }
    }

    /// Applies an in-progress thumb drag: turns the pointer's row into a
    /// thumb top within the dragged pane's track, maps that back to a content
    /// offset, and moves the pane there. Returns true when it moved.
    fn drag_to(&mut self, row: u16) -> bool {
        let Some(drag) = self.drag.as_ref() else {
            return false;
        };
        let pane = drag.pane;
        let Some(track) = self.hits.track_of(pane) else {
            return false;
        };
        let Some(spec) = self.scrollbar_spec(pane) else {
            return false;
        };
        let top = row.saturating_sub(track.y).saturating_sub(drag.grab_offset).min(track.height);
        let offset = crate::hit::offset_for_thumb_top(&spec, track.height, top);
        if offset == spec.offset {
            return false;
        }
        if self.screen == Screen::Manage {
            if self.manage.tab != ManageTab::Variables {
                return false;
            }
            self.varmanager.set_scroll(offset);
            return true;
        }
        match pane {
            PaneId::Sidebar => {
                self.sidebar.scroll = offset;
                // Dragging the viewport is an explicit gesture, exactly like
                // the wheel: the selection must not drag it back.
                self.sidebar.ensure_visible = false;
                true
            }
            PaneId::Response => {
                let moved = self.session.response.set_scroll(offset);
                if self.profile() == crate::config::KeyProfile::Vim {
                    self.session.response.cursor_into_view();
                }
                moved
            }
            PaneId::Editor => {
                // edtui owns the body's viewport and only exposes moving it
                // by one wheel notch at a time (which also keeps its cursor
                // inside the viewport); feed it the difference. A table
                // tab's view moves by exactly that many lines.
                let delta =
                    (offset as i64 - spec.offset as i64).clamp(i16::MIN as i64, i16::MAX as i64);
                self.scroll_editor(delta as i16);
                self.editor.view_offset() != spec.offset
            }
        }
    }

    /// Applies an in-progress horizontal thumb drag: turns the pointer's
    /// column into a thumb left-edge within the dragged pane's bottom
    /// track, maps that back to a column offset, and moves the pane there.
    /// The sideways twin of `drag_to`; `thumb_geometry`/`offset_for_thumb_top`
    /// are axis-agnostic lengths, so the same math serves both.
    fn drag_to_h(&mut self, column: u16) -> bool {
        let Some(drag) = self.drag.as_ref() else {
            return false;
        };
        let pane = drag.pane;
        let Some(track) = self.hits.h_track_of(pane) else {
            return false;
        };
        // Only the Response pane draws a horizontal bar today.
        let Some(spec) = (match pane {
            PaneId::Response => self.session.response.h_scrollbar_spec(),
            _ => None,
        }) else {
            return false;
        };
        let left = column.saturating_sub(track.x).saturating_sub(drag.grab_offset).min(track.width);
        let offset = crate::hit::offset_for_thumb_top(&spec, track.width, left);
        if offset == spec.offset {
            return false;
        }
        self.session.response.set_scroll_h(offset)
    }

    /// Where a one-line field's text is drawn: its left edge and width.
    /// The one place a field's geometry is read, for presses and sweeps.
    fn text_box(&self, field: FieldId) -> Option<(u16, u16)> {
        use crate::paint::WELL_PAD;
        if field == FieldId::Url {
            let area = self.editor.last_url_text_area?;
            return Some((area.x, area.width));
        }
        let area = self.hits.rect_of(&field_hit(field)?)?;
        // The jq bar's field rect is its text window, past the `jq `
        // label (`response::draw_jq_bar`), so it takes the area as it is.
        Some(match field {
            FieldId::ModalInput(_) => (area.x + 2, area.width.saturating_sub(2)),
            FieldId::VmForm(_) | FieldId::Settings(_) => {
                (area.x + WELL_PAD, area.width.saturating_sub(WELL_PAD * 2))
            }
            _ => (area.x, area.width),
        })
    }

    /// The open one-line field `field`'s input: the very cell or field
    /// named, not another one under edit. A screen field's goes through
    /// `line_buf`, the one map from a screen field to its input; a dialog's
    /// box is its form field, or the file picker's path box.
    fn field_input_mut(&mut self, field: FieldId) -> Option<&mut LineInput> {
        match field {
            FieldId::Jq => Some(&mut self.session.response.jq_bar_mut().input),
            FieldId::ModalInput(0) if matches!(self.modals.top(), Some(Modal::FilePicker(_))) => {
                self.modals.query_input_mut()
            }
            FieldId::ModalInput(i) => self.modals.text_input_mut(i),
            _ if self.editing(field) => line_buf(
                field,
                &mut self.editor.url,
                &mut self.editor.table,
                &mut self.varmanager,
                &mut self.settings,
            ),
            _ => None,
        }
    }

    /// Whether the edit open on the screen is `field` itself: the cell or
    /// form field named, not another one (the URL is always its own).
    fn editing(&self, field: FieldId) -> bool {
        match field {
            FieldId::TableCell { row, col } => self
                .editor
                .table
                .editing
                .as_ref()
                .is_some_and(|e| e.row == row && usize::from(e.col.index()) == col),
            FieldId::VmForm(f) => {
                self.varmanager.form.editing.as_ref().is_some_and(|e| e.field == f)
            }
            FieldId::VmGrid { row, col } => {
                self.varmanager.grid.editing.as_ref().is_some_and(|e| e.row == row && e.col == col)
            }
            FieldId::Settings(f) => self.settings.editing == Some(f),
            _ => true,
        }
    }

    /// The char at screen cell `(x, y)` in `field`'s text, as `(row, col)`.
    /// A one-line field maps the column through the window it drew with
    /// (`open`: from its caret, else from char 0); a sweep (`drag`) is
    /// clamped into the text, a press only from the left, and a press on a
    /// squeezed field still maps to its window's start. The body maps
    /// through the editor. `None` when the field is not open, or a sweep
    /// has no text to cover.
    fn text_at(
        &mut self,
        field: FieldId,
        x: u16,
        y: u16,
        open: bool,
        drag: bool,
    ) -> Option<(usize, usize)> {
        if field == FieldId::Body {
            return self.editor.body_char_at(x, y, drag);
        }
        let (left, width) = self.text_box(field)?;
        let col = match drag {
            true if width == 0 => return None,
            true => x.clamp(left, left + width - 1) - left,
            false => x.saturating_sub(left),
        };
        Some((0, self.field_input_mut(field)?.char_at_column(open, width.max(1), col)))
    }

    /// A left press on `field`'s text, which the field's arm has opened.
    /// Vim's mouse takes it in vim (spec 2026-10-14). Otherwise the caret
    /// goes to the char and anchors a sweep, a double click selects the
    /// word and the sweep goes by words, and a triple click selects the line
    /// and the sweep goes by lines (spec 2026-10-14, align the text mouse,
    /// the same shapes as vim's). A sweep is armed when the press
    /// maps to a char (the body arms one anyway; its own mouse code
    /// decides); a field that is not open, which a commit can leave so,
    /// places and arms nothing.
    fn press_text(
        &mut self,
        field: FieldId,
        m: ratatui::crossterm::event::MouseEvent,
        clicks: Clicks,
        open: bool,
    ) {
        let at = self.text_at(field, m.column, m.row, open, false);
        if field == FieldId::Body || at.is_some() {
            self.text_drag = Some(TextDrag::Field(field));
        }
        // A press on the body's text gives it the caret, as its own mouse
        // code does: vim takes the press only on the field with the caret.
        if field == FieldId::Body && at.is_some() {
            self.move_editor_caret(SubFocus::Content);
        }
        if let Some(at) = at
            && self.vim_text_press(field, at, clicks.text)
        {
            return;
        }
        if field == FieldId::Body {
            self.editor.handle_mouse(m);
            if clicks.text == 2 {
                self.editor.body_select_word_at(m.column, m.row);
            } else if clicks.text == 3 {
                self.editor.body_select_line_at(m.column, m.row);
            }
            return;
        }
        let (Some(at), Some(input)) = (at, self.field_input_mut(field)) else { return };
        match clicks.text {
            2 => input.select_word_at(at.1),
            3 => input.select_line(),
            _ => {
                input.set_cursor(at.1);
                input.begin_mouse_selection();
            }
        }
    }

    /// The sweep a press armed on `field`'s text moved to `(x, y)`.
    fn drag_text(&mut self, field: FieldId, x: u16, y: u16) -> bool {
        let Some(at) = self.text_at(field, x, y, true, true) else { return false };
        if self.vim_text_drag(field, at) {
            return true;
        }
        if field == FieldId::Body {
            return self.editor.body_drag_to(x, y);
        }
        match self.field_input_mut(field) {
            Some(input) => {
                input.extend_mouse_selection_to(at.1);
                true
            }
            None => false,
        }
    }

    /// Commits any in-progress table cell edit, surfacing its warning as a
    /// toast. The one place typing is turned into map data outside the
    /// table's own key handling: the caret leaving the cell
    /// (`capture_undo`), click-away, tab switch, save, send, every dirty
    /// gate (`leaving_unsaved`) and every path that replaces the editor
    /// (`flush_field_session`) go through here, so a typed cell is never
    /// dropped and nothing outside the table ever reads an uncommitted
    /// draft (spec 2026-10-19).
    ///
    /// The commit is captured here as an undo step of its own (task #66):
    /// the main loop captures once per event, so without it whatever the
    /// same event does next (a click on another row's toggle, a click away
    /// whose action edits) would join the commit, and one undo would take
    /// the typing back with it. What was already pending is captured
    /// first, so the commit is bracketed on both sides: a change its caller
    /// made before committing (a method change whose tab sync commits the
    /// cell) is its own step before it. It lives here rather than in the
    /// click dispatch because keyboard paths commit through here too.
    ///
    /// No field session holds these captures back: a cell is open only
    /// while the caret is in it, so the URL line and the jq bar never hold
    /// keystrokes beside an open cell. The assertion holds the suite to
    /// that; were it broken, the commit would join the field's close step
    /// (the merge before #66), never lose the typing.
    ///
    /// `no_coalesce` belongs to a change already made: the capture before
    /// the commit records that change and spends the flag with it. A caller
    /// that commits and then makes a wholesale change sets the flag after
    /// committing, as every caller does (a method change sets it, changes
    /// the method, then its tab sync commits).
    pub(crate) fn commit_table_edit(&mut self) {
        if self.editor.table.editing.is_none() {
            return;
        }
        debug_assert!(!self.field_gate(), "a table cell open beside a field session");
        self.capture_step();
        if let Some(w) = self.editor.commit_table().warning {
            self.toasts.push(w, ToastKind::Warning);
        }
        self.capture_step();
    }

    /// Moves the editor's caret for a click's dispatch, left or right. A URL
    /// line it leaves closes here, so its typing is recorded as its own step
    /// before the click acts (see [`App::leave_text_field`]); a click that
    /// keeps it open never comes here, and its typing stays one step with
    /// its own history intact. The editor's own body mouse code also places
    /// the caret, but a body press edits nothing, so a close it makes lands
    /// alone at the end of the event.
    pub(super) fn move_editor_caret(&mut self, to: SubFocus) {
        if self.focus == PaneId::Editor && self.editor.sub_focus != to {
            self.leave_text_field();
        }
        self.editor.sub_focus = to;
    }

    /// Commits the in-progress edit and re-resolves row `i` — an index from
    /// the last frame's hit map — afterwards. A duplicate-key commit
    /// `shift_remove`s a row, shifting every later index down, so acting on
    /// the raw `i` would hit the neighbour. `None` when the row the user
    /// clicked no longer exists (it was the one collapsed away).
    fn resolve_table_row_across_commit(&mut self, i: usize) -> Option<usize> {
        let edited_row = self.editor.table.editing.as_ref().map(|e| e.row);
        let key = self.editor.table_key_at(i);
        self.commit_table_edit();
        match edited_row {
            // Nothing was being edited: the index is still good.
            None => Some(i),
            // The clicked row is the one that just committed — its key may
            // have changed under us, so take the row the commit resolved to
            // (a discarded ghost row resolves to nothing to act on).
            Some(r) if r == i => match self.editor.table.selected() {
                Some(Stop::Row(row)) => Some(row),
                _ => None,
            },
            // Another row committed; that commit may have collapsed rows,
            // so re-resolve by the key `i` named before it ran.
            Some(_) => match key {
                Some(k) => self.editor.table_index_of(&k),
                // The ghost row has no key; it sits at the new length.
                None => Some(self.editor.table_len()),
            },
        }
    }

    /// A left click landing on response body text: a single click anchors
    /// a cell-wise sweep, a double click selects the word under it (and
    /// arms a word-wise sweep). Returns whether anything anchored — the
    /// caller arms `TextDrag::Response` on `true`.
    fn response_click_selection(
        &mut self,
        clicks: u8,
        m: ratatui::crossterm::event::MouseEvent,
    ) -> bool {
        if clicks == 2 {
            self.session.response.select_word_at(m.column, m.row)
        } else {
            self.session.response.begin_selection_at(m.column, m.row)
        }
    }

    /// A left press on the response pane: its bare content, or its header
    /// strip, which is the split handle and clicks first (spec E2a). The
    /// one path for both, so the strip's click can't drift from the
    /// pane's.
    ///
    /// A press on the bare content (Raw and Headers register no per-row
    /// hits) anchors a selection and puts the cursor on the clicked char,
    /// exactly like a JsonRow click does in Pretty (spec 2026-10-22,
    /// matching brackets, R18). A double click too, as Pretty's does: the
    /// cursor takes the word's start, where the anchor is. R18 asks for
    /// one path for all three views, so this matches Pretty rather than a
    /// view of its own. A click below the text anchors on the last line's
    /// end (`cell_at`'s clamp, as the selection's anchor always has), so
    /// the cursor goes there, as an editor's caret does. A press off the
    /// text (the strip) anchors nothing but still clears the selection and
    /// the lit bracket pair. `sweep` arms the selection sweep on an
    /// anchor; the strip passes `false`, as its motion is the resize's.
    /// Either way the response takes the focus. Returns whether anything
    /// changed.
    fn press_response_pane(
        &mut self,
        clicks: u8,
        m: ratatui::crossterm::event::MouseEvent,
        sweep: bool,
    ) -> bool {
        let anchored = self.response_click_selection(clicks, m);
        let mut changed = anchored;
        if anchored {
            if sweep {
                self.text_drag = Some(TextDrag::Response);
            }
            if let Some(row) = self.session.response.anchor_line() {
                changed |= self.update(Action::JsonRowClicked { row, toggle: false });
            }
        }
        self.update(Action::FocusPane(PaneId::Response)) || changed
    }

    /// The `▼`/`▲` search buttons: focus the response pane and step the
    /// match cycle, exactly as `n`/`N` do.
    fn step_response_search(&mut self, delta: i32) -> bool {
        self.update(Action::FocusPane(PaneId::Response));
        self.with_response_search(|response, memory| response.step_search(delta, memory));
        self.update(Action::Render)
    }

    /// Mirrors every keybound (`keys::named_actions`) action that `on_hit`
    /// below dispatches directly — i.e. reachable by a click that isn't
    /// already counted via a footer/toolbar chip, a context menu, or a
    /// palette command. Feeds `app::tests`'s mouse-parity sweep (spec §5):
    /// that test can't reflect over `on_hit`'s match arms, so this list is
    /// hand-kept beside it instead. Add a `Hit` arm below that fires a
    /// keybound action directly, and add the same action here in the same
    /// change — otherwise the parity test starts failing for it.
    #[cfg(test)]
    pub(crate) fn mouse_dispatch_mirror() -> Vec<Action> {
        vec![
            Action::Close,                    // Hit::ModalOutside
            Action::OpenProjectChooser,       // Hit::HeaderProject
            Action::CycleProject(1),          // Hit::HeaderProjectCycle
            Action::OpenSpaceChooser,         // Hit::HeaderSpace
            Action::CycleSpace(1),            // Hit::HeaderSpaceCycle
            Action::OpenEnvChooser,           // Hit::HeaderEnv
            Action::CycleEnv(1),              // Hit::HeaderEnvCycle
            Action::OpenManage { tab: None }, // Hit::HeaderManage
            Action::OpenThemeChooser,         // Hit::HeaderTheme
            Action::OpenMethodDropdown,       // Hit::MethodSelector
            Action::FocusUrl,                 // Hit::UrlBar
            Action::Send,                     // Hit::SendButton (not in flight)
            Action::EditorTabSelect(0),       // Hit::EditorTab, any draw position
            Action::EditorTabSelect(1),       // (converted through
            Action::EditorTabSelect(2),       //  EditorTab::from_draw_position(..).index())
            Action::EditorTabSelect(3),
            // The Environments/Spaces tabs' `+ New` button (Hit::ManageNew);
            // the rest of that face's hits either dispatch per-item actions
            // built from the live project (rename/delete/move/move-all) or
            // change list state only.
            Action::OpenNewEnvPrompt,
            Action::OpenNewSpacePrompt,
        ]
    }

    /// Whether a left press on `hit` lands on a window separator (spec S7):
    /// it moves a divider and nothing else, so it clicks nothing away and
    /// moves no focus. The sidebar gutter is one in both profiles, since a
    /// click there never did anything. In vim the response's header strip
    /// is one too: it is the editor window's status line, and Vim 9.1 keeps
    /// the current window and Visual mode through a press on a status line
    /// (probed with `test_setmouse`, both the current window's and the one
    /// above it). Outside vim the strip is the response pane's header, and
    /// a press there clicks first (E2a).
    ///
    /// The strip's control band (task #95) is the status line's press
    /// without its drag in vim too. A Vim status line drags anywhere, but
    /// it holds no buttons, so it has no rule for the cells between them;
    /// the strip's buttons already don't drag in vim (E2), and the band is
    /// the area around them, so it follows the buttons in both profiles.
    fn is_separator_press(&self, hit: &Hit) -> bool {
        match hit {
            Hit::SidebarGutter => true,
            Hit::SplitHandle | Hit::StripControls => {
                self.profile() == crate::config::KeyProfile::Vim
            }
            _ => false,
        }
    }

    /// The press on a resize handle or the strip's control band, before
    /// any drag. A press on a window separator
    /// ([`Self::is_separator_press`]) moves no focus and clicks nothing.
    /// Otherwise (the strip, outside vim) it is the response pane's header,
    /// so its press clicks as the pane's does (spec E2a; the click-away in
    /// [`Self::on_hit`] already ran for it), arming no text sweep: the
    /// motion is the resize's, or nothing's.
    fn press_handle(&mut self, hit: &Hit, clicks: u8, m: ratatui::crossterm::event::MouseEvent) {
        if self.is_separator_press(hit) {
            self.vim_separator_press();
        } else {
            self.press_response_pane(clicks, m, false);
        }
    }

    /// The central click dispatch: maps a resolved `Hit` (plus click count
    /// and the raw event, for hits that need to forward it) to app state
    /// changes. Only `Pane` and `BodyEditor` are wired up so far; later
    /// tasks extend this match as more hit kinds gain behavior.
    fn on_hit(
        &mut self,
        hit: Hit,
        clicks: Clicks,
        m: ratatui::crossterm::event::MouseEvent,
    ) -> bool {
        // A click in a dialog is the dialog's, as its keys are, for the
        // request editor, the Settings tab (task #73) and the Variable
        // Manager (task #75): while one is open their click-away rules
        // don't run, so the URL line, a table cell, a Settings field or a
        // Manager input under it stays open, whether the click lands
        // on its buttons, its text box, its footer chips or a gap between
        // them. The rule asks whether a dialog is open, as the jq bar's
        // rule and the no-hit point do, rather than listing the dialogs'
        // hits, so there is no dialog hit left to miss. It reads the stack
        // while the hit comes from the last frame, which is what every
        // click path relies on: the loop redraws after any event that
        // reports a change.
        // (The footer's quit chip, once its exception, is not drawn under a
        // dialog since task #85: no quit runs under one.)
        let in_dialog = !self.modals.is_empty();
        // A click anywhere that isn't the params/headers table itself (and
        // isn't in a dialog) is a click away: it commits whatever cell was
        // being edited (typing is never silently thrown away) and clears
        // the table selection. These are the hits that do neither.
        let keeps_open_cell = in_dialog
            || matches!(
                hit,
                Hit::TableRow(_)
                    | Hit::TableCheckbox(_)
                    | Hit::TableCopy(_)
                    | Hit::TableDelete(_)
                    | Hit::TableCell { .. }
                    // The auto row's pencil commits an open cell itself, as
                    // its own undo step, before it adds its row.
                    | Hit::AutoHeaderOverride(_)
                    | Hit::SplitStop(_)
                    | Hit::ScrollbarThumb(_)
                    | Hit::ScrollbarTrack(..)
                    | Hit::HScrollThumb(_)
                    | Hit::HScrollTrack(..)
                    // The `{{ }} vars` chip inserts into whatever text field
                    // is live, so it must not be treated as a click away:
                    // blurring the URL line (or committing the cell) first
                    // would leave the picker it opens with nowhere to insert.
                    | Hit::FooterChip(Action::OpenVarPicker { .. })
                    // The open tooltip's edit chip is its 󰏫, whose click
                    // keeps the cell open too (#135); committing it first
                    // would take the caret's tooltip down before it acts.
                    | Hit::FooterChip(Action::TipPill(_))
            )
            // The tooltip floats over the rows: clicking one of its pills
            // must neither commit the cell under edit nor drop the selection.
            || hit.is_float_overlay()
            // A press on a window separator moves a divider and nothing
            // else.
            || self.is_separator_press(&hit);
        // Keeping the cursor is a separate question from keeping a cell
        // open. The auto rows are the table's stops too (task #56): their
        // copy pill and the section's reveal toggle are side actions, so
        // they keep the cursor that may be sitting on one, but they are
        // still a click away for an open cell, which commits first, as on
        // main. A press on the rest of the section is the bare pane's, as
        // before.
        let keeps_table_selection =
            keeps_open_cell || matches!(hit, Hit::AutoHeaderCopy(_) | Hit::AutoHeaderReveal);
        if !keeps_open_cell {
            self.commit_table_edit();
        }
        if !keeps_table_selection {
            self.editor.table.set_selected(None);
        }
        // Same rule for the Variable Manager's input, the form field or the
        // grid cell: any click that isn't on the input itself (or the
        // reveal toggle, which must not disturb an edit in progress) closes
        // it first, saved — including a click on a *different* field or
        // cell, which must never silently overwrite the edit that was live
        // (Task 8's commit-first rule). A close the input refuses swallows
        // the click, as the Settings field's does, so nothing the click
        // would do runs over the typing.
        //
        // Under a dialog the input stays open, as the table's does: a
        // click in the dialog is the dialog's (its Copy/Paste menu acts on
        // the input, task #76), and a command the dialog runs that changes
        // the state the input was typed against closes it first itself
        // (`close_open_inputs`, task #75).
        let on_the_input = matches!(hit, Hit::VmRevealToggle)
            && self.varmanager.form.editing.is_some()
            || matches!(hit, Hit::VmFormField(field)
                if self.varmanager.form.editing.as_ref().is_some_and(|e| e.field == field))
            || matches!(hit, Hit::VmEntryCell { row, col }
                if self.varmanager.grid.editing.as_ref().is_some_and(|e| e.row == row && e.col == col));
        if !in_dialog && !on_the_input && !self.close_manager_input() {
            return self.update(Action::Render);
        }
        // …and the Settings tab's field and cursor, likewise. Any click
        // that isn't on one of the tab's own controls commits whatever
        // was being typed *and* drops the keyboard cursor: with no
        // selection band, the cursor is a control lifting its own fill,
        // and one left lifted after the click has landed elsewhere says
        // "type here" about a row that will no longer answer. A commit
        // the field refuses swallows the click and keeps both, exactly
        // as it does on the tab's own hits.
        //
        // Overlays are exempt, like every other click-away rule here: a
        // dialog, a dropdown or a scrollbar drawn over the tab must not
        // pull the caret out from under what is being typed.
        // `keeps_table_selection` already holds those exemptions.
        let keeps_settings_focus = keeps_table_selection
            || matches!(
                hit,
                Hit::SettingsRow(_)
                    | Hit::SettingsControl(_)
                    | Hit::SettingsJqTab(_)
                    | Hit::SettingsKeymap(_)
                    | Hit::SettingsIndent(_)
                    | Hit::SettingsFile { .. }
            );
        if !keeps_settings_focus {
            if self.settings.editing.is_some() && !self.commit_settings_edit_for_click() {
                return true;
            }
            self.settings.focused = false;
        }
        // Likewise, clicking away blurs whichever editor input is active
        // (URL line / table / body). Hits that themselves place the
        // sub-focus (UrlBar, BodyEditor, the table hits) re-set it right
        // after; dialog and scrollbar clicks are excluded above so an open
        // popup or a scroll never blurs the input under it.
        let keeps_editor_input =
            keeps_table_selection || matches!(hit, Hit::UrlBar | Hit::BodyEditor | Hit::CopyUrl);
        if !keeps_editor_input {
            self.move_editor_caret(SubFocus::None);
        }
        match hit {
            Hit::Pane(PaneId::Response) => self.press_response_pane(clicks.text, m, true),
            Hit::Pane(p) => self.update(Action::FocusPane(p)),
            Hit::BodyEditor => {
                self.update(Action::FocusPane(PaneId::Editor));
                // The caret, a double click's word, and a sweep armed:
                // whole words after a double click.
                self.press_text(FieldId::Body, m, clicks, true);
                self.update(Action::Render)
            }
            Hit::HeaderProject => self.update(Action::OpenProjectChooser),
            Hit::HeaderProjectCycle => self.update(Action::CycleProject(1)),
            Hit::HeaderSpace => self.update(Action::OpenSpaceChooser),
            Hit::HeaderSpaceCycle => self.update(Action::CycleSpace(1)),
            Hit::HeaderEnv => self.update(Action::OpenEnvChooser),
            Hit::HeaderEnvCycle => self.update(Action::CycleEnv(1)),
            Hit::HeaderTheme => self.update(Action::OpenThemeChooser),
            Hit::HeaderManage => self.update(Action::OpenManage { tab: None }),
            Hit::ManageTab(i) => self.update(Action::SelectManageTab(
                crate::components::manage::ManageTab::from_index(i),
            )),
            Hit::ManageRow(i) => {
                self.manage.list.cursor = i;
                self.manage.list.leave_detail();
                // A press on a Spaces or Environments row also arms a
                // possible row drag; the click itself still just selects
                // (the drag only starts once the pointer leaves the row).
                if let Some(name) = self.manage_selected(self.manage.tab) {
                    self.manage_press = Some((i, name));
                }
                self.update(Action::Render)
            }
            Hit::ManageNew => self.update(match self.manage.tab {
                ManageTab::Spaces => Action::OpenNewSpacePrompt,
                _ => Action::OpenNewEnvPrompt,
            }),
            Hit::ManageRename => {
                let tab = self.manage.tab;
                match self.manage_selected(tab) {
                    Some(name) => self.update(
                        crate::components::manage_list::ManageList::rename_action(tab, &name),
                    ),
                    None => false,
                }
            }
            Hit::ManageDelete => {
                let tab = self.manage.tab;
                match self.manage_selected(tab) {
                    Some(name) if tab == ManageTab::Spaces => {
                        self.update(Action::DeleteSpace(name))
                    }
                    Some(name) => self.update(Action::DeleteEnv(name)),
                    None => false,
                }
            }
            // -- Settings tab --
            // A click anywhere on the tab commits whatever field was
            // under edit first: typing is never silently thrown away,
            // the same rule the params/headers table follows. A commit
            // the field *refuses* swallows the click instead -- see
            // `commit_settings_edit_for_click`.
            Hit::SettingsRow(i) => {
                if !self.commit_settings_edit_for_click() {
                    return true;
                }
                self.settings.focused = true;
                self.settings.set_cursor(i);
                self.update(Action::Render)
            }
            // A click on a control: commit whatever other field was live
            // (a commit the field *refuses* swallows the click and keeps
            // the typed text), then open this one and put the caret
            // where the pointer is.
            //
            // Clicking the well of the field you are *already* editing
            // commits nothing: committing and reopening would select-all
            // over a rejected value and hand back the stored one. It
            // only moves the caret, which is what a click in a live text
            // box means everywhere else in the app.
            Hit::SettingsControl(field) => {
                let already_editing = self.settings.editing == Some(field);
                if !already_editing {
                    if !self.commit_settings_edit_for_click() {
                        return true;
                    }
                    self.settings.focus_field(field);
                    // Called for the effect, not the answer: this is
                    // what ticks the checkbox, switches the jq segment
                    // or opens the text field's edit.
                    self.activate_settings_row();
                }
                // Only a text row has a well and a caret to place. A
                // checkbox or a jq segment was toggled by
                // `activate_settings_row` above, opens no edit, and must
                // arm no sweep — one would then follow the pointer
                // across a row holding no text.
                if self.settings.editing == Some(field) {
                    self.press_text(FieldId::Settings(field), m, clicks, already_editing);
                }
                self.update(Action::Render)
            }
            Hit::SettingsJqTab(mode) => {
                use crate::components::settings::{SettingsField, jq_tab_spelling};
                if !self.commit_settings_edit_for_click() {
                    return true;
                }
                self.settings.focus_field(SettingsField::JqTab);
                self.update(Action::SetUiString {
                    key: SettingsField::JqTab.key(),
                    value: jq_tab_spelling(mode).to_string(),
                })
            }
            Hit::SettingsKeymap(profile) => {
                use crate::components::settings::SettingsField;
                if !self.commit_settings_edit_for_click() {
                    return true;
                }
                self.settings.focus_field(SettingsField::Keymap);
                self.update(Action::SetKeyProfile(profile))
            }
            Hit::SettingsIndent(kind) => {
                use crate::components::settings::SettingsField;
                if !self.commit_settings_edit_for_click() {
                    return true;
                }
                self.settings.focus_field(SettingsField::Indent);
                self.update(Action::SetUiString {
                    key: SettingsField::Indent.key(),
                    value: kind.spelling().to_string(),
                })
            }
            Hit::SettingsFile { file, reset } => {
                use crate::components::settings::{SettingsRow, SettingsTab};
                if !self.commit_settings_edit_for_click() {
                    return true;
                }
                self.settings.focused = true;
                if let Some(i) =
                    SettingsTab::rows().iter().position(|r| *r == SettingsRow::File(file))
                {
                    self.settings.set_cursor(i);
                }
                self.settings.file_button = usize::from(reset);
                self.activate_settings_row()
            }
            Hit::ManageEnvTls(policy) => match self.manage_selected(ManageTab::Environments) {
                Some(env) => self.update(Action::SetEnvTls { env, policy }),
                None => false,
            },
            Hit::ManageMoveAll => match self.manage_selected(ManageTab::Spaces) {
                Some(from) => self.update(Action::PromptMoveAllRequests(from)),
                None => false,
            },
            // The quit chip is a quit request: it hides under a dialog, and
            // a click resolved against an older frame is refused there as
            // the other requests are (task #85, `quit_refused`).
            Hit::FooterChip(action) if action.quits_under_a_dialog() && self.quit_refused() => {
                self.refuse_quit()
            }
            Hit::FooterChip(action) => {
                // Chips that live inside the request panel (the Body
                // toolbar row, the address bar's TLS lock) focus the panel
                // like any of its buttons. The footer's own chips are
                // contextual — they describe the pane already focused — so
                // they never move focus themselves.
                if matches!(
                    action,
                    Action::FormatBody
                        | Action::MinifyBody
                        | Action::ToggleBodyVars
                        | Action::OpenBodyInEditor
                        | Action::BodyClear
                        | Action::ToggleInsecure
                ) {
                    self.update(Action::FocusPane(PaneId::Editor));
                }
                self.update(action)
            }
            Hit::SidebarNewRequest => self.update(Action::PromptNewRequest),
            Hit::SidebarImportCurl => {
                self.update(Action::ImportCurl { folder: self.sidebar_import_folder() })
            }
            Hit::SidebarFolderArrow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.set_sidebar_selected(i);
                self.update(Action::ToggleSelectedFolder)
            }
            Hit::SidebarRow(i) => {
                self.update(Action::FocusPane(PaneId::Sidebar));
                self.set_sidebar_selected(i);
                match self.sidebar.rows.get(i).cloned() {
                    // A press on a request row also arms a possible row
                    // drag; the click itself still opens the request (the
                    // drag only starts once the pointer leaves the row).
                    Some(Row::Request { slug, broken: None, .. }) => {
                        self.sidebar_press = Some((i, slug.clone()));
                        self.update(Action::OpenRequest(slug))
                    }
                    Some(Row::Request { slug, broken: Some(_), .. }) => {
                        self.sidebar_press = Some((i, slug.clone()));
                        self.update(Action::ShowRequestError(slug))
                    }
                    Some(Row::Folder { .. }) => {
                        if clicks.list == 2 {
                            self.update(Action::ToggleSelectedFolder)
                        } else {
                            self.update(Action::Render)
                        }
                    }
                    None => false,
                }
            }
            Hit::EditorTab(i) => {
                // `i` is the tab's on-screen (draw-order) position — Params,
                // Headers, Vars, Body — which is not the same numbering as
                // `EditorTabSelect`'s stable index (kept unchanged so the
                // bound `editor_tab_N` numbers still land on
                // Params/Headers/Body); convert.
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::EditorTabSelect(EditorTab::from_draw_position(i).index()))
            }
            Hit::SendButton => {
                self.update(Action::FocusPane(PaneId::Editor));
                if self.session.is_in_flight(&self.editor.slug) {
                    self.update(Action::CancelSend)
                } else {
                    self.update(Action::Send)
                }
            }
            Hit::TableCheckbox(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.move_editor_caret(SubFocus::Content);
                // A checkbox click during another row's edit commits that
                // edit first — and that commit can collapse rows, so `i`
                // (baked into the last frame's hit map) is re-resolved by
                // the key it named before the toggle lands.
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                // A toggle click is just a toggle — it must not select (or
                // expand) the row.
                let map = match self.editor.active_tab {
                    EditorTab::Params => &mut self.editor.params,
                    EditorTab::Headers => &mut self.editor.headers,
                    EditorTab::Vars => &mut self.editor.variables,
                    EditorTab::Body => {
                        unreachable!("TableCheckbox only fires on Params/Headers/Vars")
                    }
                };
                if let Some((_, e)) = map.get_index_mut(i) {
                    e.enabled = !e.enabled;
                }
                self.update(Action::Render)
            }
            // The row background is only reachable at the slivers its cells
            // don't cover (the accent-bar column, the key/value divider):
            // it selects the row, nothing more — editing is the cells' job.
            Hit::TableRow(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.move_editor_caret(SubFocus::Content);
                // The background of the row being edited is that row's own
                // chrome (the pad lines its expansion added): clicking it
                // is inert, so the second click of a double click can't
                // cancel the edit the first one started.
                if self.editor.table.editing.as_ref().is_some_and(|e| e.row == i) {
                    return self.update(Action::Render);
                }
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                let len = self.editor.table_len();
                self.editor.table.set_selected(Some(Stop::of_row(i, len)));
                self.update(Action::Render)
            }
            // A cell click edits that cell in place, committing whatever
            // was being edited before it.
            Hit::TableCell { row, col } => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.move_editor_caret(SubFocus::Content);
                let cell_col = crate::components::table_editor::Col::from_index(col);
                let already_editing = self
                    .editor
                    .table
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == row && e.col == cell_col);
                if !already_editing {
                    let outcome = self.editor.click_table_cell(row, cell_col);
                    if let Some(w) = outcome.warning {
                        self.toasts.push(w, ToastKind::Warning);
                    }
                }
                // The caret at the clicked char (the cell draws windowed
                // at the hit rect's own left edge, so a value longer than
                // its cell may be scrolled), a double click's word, and a
                // sweep armed. Nothing when a commit collapsed rows out
                // from under the click: the edit then isn't the cell the
                // pointer was on, so the seeded caret-at-end stays.
                self.press_text(
                    FieldId::TableCell { row, col: usize::from(col) },
                    m,
                    clicks,
                    already_editing,
                );
                self.update(Action::Render)
            }
            Hit::TableCopy(i) => {
                // The copy pill shows on the row being typed into, so it
                // has to commit that edit first — otherwise the clipboard
                // gets the value the screen has already stopped showing —
                // and re-resolve `i`, since the commit can collapse rows.
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                let target = if alt_click(&m) && self.editor.active_tab == EditorTab::Headers {
                    CopyTarget::TableHeaderLine(i)
                } else {
                    CopyTarget::TableRow(i)
                };
                self.update(Action::CopyToClipboard(target))
            }
            Hit::TableDelete(i) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.move_editor_caret(SubFocus::Content);
                // Same as the checkbox: commit first, then re-resolve the
                // row so the confirm never names the wrong one.
                let Some(i) = self.resolve_table_row_across_commit(i) else {
                    return self.update(Action::Render);
                };
                self.update(Action::DeleteTableRow(i))
            }
            Hit::SplitStop(stop) => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::SplitStop(stop))
            }
            // The strip's control band (task #95): the strip's press without
            // the drag.
            Hit::StripControls => {
                self.press_handle(&hit, clicks.text, m);
                self.update(Action::Render)
            }
            // A press on a resize handle arms the drag, after the handle's
            // own press ([`Self::press_handle`]). The handle is the hovered
            // hit for the length of the drag, so it stays lit wherever the
            // pointer goes.
            Hit::SidebarGutter | Hit::SplitHandle => {
                self.press_handle(&hit, clicks.text, m);
                let kind = if hit == Hit::SidebarGutter {
                    ResizeHandle::Sidebar
                } else {
                    // Where this frame drew the strip, not where it will
                    // settle: the user grabs what they see. The first motion
                    // snaps the split (`set_split_now`), so from then on
                    // drawn and settled agree, and measuring from the drawn
                    // strip keeps it under the pointer (spec E5). Measuring
                    // from the settled strip would put the line the ease's
                    // remaining rows away.
                    let top = self.hits.rect_of(&Hit::SplitHandle).map_or(m.row, |r| r.y);
                    ResizeHandle::Split {
                        grab: m.row.saturating_sub(top),
                        press: m.row,
                        start: self.split_state(),
                    }
                };
                self.resize = Some(ResizeDrag { kind, moved: false, tracked: false });
                if self.hovered.as_ref() != Some(&hit) {
                    self.hovered = Some(hit);
                    self.begin_hover_fade();
                }
                self.update(Action::Render)
            }
            Hit::UrlBar => {
                let was_focused = self.editor.url_open();
                // `Action::FocusUrl` is exactly "focus Editor, sub-focus
                // Url" (see its handler) — dispatching it here rather than
                // setting both fields by hand is what makes the mouse-parity
                // test's job possible: `focus_url` genuinely goes through
                // this arm rather than merely resembling it.
                self.update(Action::FocusUrl);
                // The drawn window starts at 0 unfocused, or follows the
                // caret when already focused.
                self.press_text(FieldId::Url, m, clicks, was_focused);
                self.update(Action::Render)
            }
            Hit::MethodSelector => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::OpenMethodDropdown)
            }
            Hit::DropdownRow(i) => {
                let Some(Modal::Dropdown(state)) = self.modals.top_mut() else {
                    return false;
                };
                // A disabled row swallows its click: nothing runs, and the
                // menu stays open rather than closing on a dead press.
                let Some(action) = state.items.get(i).and_then(|it| it.action.clone()) else {
                    return false;
                };
                // Closed as Enter on the row closes it, housekeeping and all
                // (a parked field's session made live, the sidebar's
                // right-click selection settled).
                self.close_dropdown_and_run(action)
            }
            // A row's own button (jq views spec 17): it runs once the menu
            // is gone, as the row's own action does.
            Hit::DropdownRowButton { row, verb } => {
                let Some(action) = self.modals.dropdown_row_action(row, verb) else { return false };
                self.close_dropdown_and_run(action)
            }
            Hit::ModalOutside => {
                if !self.modals.top_is_dismissable() {
                    // `Modal::ConfigStartup` blocks until answered --
                    // same rule `ModalStack::handle_key` already applies
                    // to `Esc` for it.
                    return false;
                }
                // The option Edit prompt is an editing surface, not a
                // question: clicking off it SAVES, like the grid's
                // commit-on-click-away. Routed through the same path Enter
                // takes so a refused commit keeps the prompt (and the typed
                // text) exactly as the keyboard flow would.
                //
                // The theme picker too: its highlight is already applied
                // live, so the theme on screen when the user clicks away
                // is the one they chose — clicking off keeps it (Enter),
                // where Esc is the explicit "put it back".
                let theme_picker = self.theme_preview.is_some()
                    && matches!(
                        self.modals.top(),
                        Some(crate::components::modal::Modal::Chooser(_))
                    );
                if theme_picker {
                    let enter = ratatui::crossterm::event::KeyEvent::new(
                        ratatui::crossterm::event::KeyCode::Enter,
                        ratatui::crossterm::event::KeyModifiers::NONE,
                    );
                    if let Some(res) = self.modals.handle_key(enter) {
                        return self.apply_modal_result(res);
                    }
                    return true;
                }
                if matches!(
                    self.modals.top(),
                    Some(crate::components::modal::Modal::MultiPrompt {
                        kind: crate::components::modal::PromptKind::EditOption { .. },
                        ..
                    })
                ) {
                    // `MultiPrompt`'s own Enter arm only closes a field to
                    // selected now (spec 2026-09-16) — submitting is
                    // `confirm_top`'s job alone, same path the button
                    // row's Confirm click takes.
                    if let Some(res) = self.modals.confirm_top() {
                        return self.apply_modal_result(res);
                    }
                    return true;
                }
                // A one-field prompt cancels exactly as Esc does: a curl
                // import prompt skips its command (the bare pop below would
                // end the whole batch), the send-time secret prompt says
                // the send was canceled.
                if matches!(self.modals.top(), Some(Modal::Prompt { .. })) {
                    let Some(res) = self.modals.cancel_top() else {
                        return false;
                    };
                    return self.apply_modal_result(res);
                }
                self.update(Action::Close)
            }
            // A click on the modal's own chrome (body/borders/query line)
            // — not one of its interactive hits, which register on top and
            // so win first. Inert: neither closes the modal nor dispatches
            // anything.
            Hit::ModalBody => false,
            Hit::ModalField(i) => {
                // A click into the modal's body takes the keyboard off
                // the button row, even when the field it lands on has no
                // text box of its own to focus.
                self.modals.leave_button_row();
                if let Some(crate::components::modal::Modal::MultiPrompt {
                    focus,
                    fields,
                    kind,
                    ..
                }) = self.modals.top_mut()
                {
                    *focus = i;
                    // A fixed-choice field has no caret to place — clicking
                    // it advances to the next choice (wrapping), so the
                    // mouse can drive it without knowing about ←/→. The
                    // same kind-specific follow-up as the keyboard cycle
                    // runs after it (the value popup reseeds its value
                    // box from what the new scope stores).
                    if let Some(field) = fields.get_mut(i)
                        && !field.choices.is_empty()
                    {
                        field.cycle(1);
                        crate::components::modal::resync_after_choice_cycle(kind, fields);
                    }
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalChoiceArrow { field, dir } => {
                self.modals.leave_button_row();
                if let Some(crate::components::modal::Modal::MultiPrompt {
                    focus,
                    fields,
                    kind,
                    ..
                }) = self.modals.top_mut()
                {
                    *focus = field;
                    if let Some(f) = fields.get_mut(field)
                        && !f.choices.is_empty()
                    {
                        f.cycle(dir.into());
                        crate::components::modal::resync_after_choice_cycle(kind, fields);
                    }
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalInput(i) => {
                let was_focused = self.modals.focused_input_index() == Some(i);
                if self.modals.focus_input(i).is_none() {
                    return false;
                }
                // Through the window the field drew with at click time
                // (unfocused fields draw from 0).
                self.press_text(FieldId::ModalInput(i), m, clicks, was_focused);
                self.update(Action::Render)
            }
            Hit::ModalRowToggle(i) => {
                self.modals.leave_button_row();
                if let Some(crate::components::modal::Modal::FieldsEditor(state)) =
                    self.modals.top_mut()
                {
                    state.toggle(i);
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalAddRow => {
                self.modals.leave_button_row();
                if let Some(crate::components::modal::Modal::FieldsEditor(state)) =
                    self.modals.top_mut()
                {
                    state.add_row();
                    return self.update(Action::Render);
                }
                false
            }
            // The mouse twin of ctrl+r: flips the mask and nothing else
            // (focus stays wherever it was, so typing carries on).
            Hit::ModalRevealToggle => {
                if let Some(crate::components::modal::Modal::Prompt { revealed, .. }) =
                    self.modals.top_mut()
                {
                    *revealed = !*revealed;
                }
                self.update(Action::Render)
            }
            Hit::ModalSharedToggle => {
                self.modals.leave_button_row();
                if let Some(crate::components::modal::Modal::Prompt {
                    kind: crate::components::modal::PromptKind::NewSelector { shared, on_toggle },
                    ..
                }) = self.modals.top_mut()
                {
                    *shared = !*shared;
                    // Clicking the row focuses it too, so space keeps
                    // working from where the mouse left off.
                    *on_toggle = true;
                    return self.update(Action::Render);
                }
                false
            }
            Hit::ModalRemove => self.remove_from_value_popup(),
            // The painted buttons dispatch the modal's own cancel/confirm
            // directly; a synthesized Esc would only close the field now.
            // Message's only button ("OK") also maps to `ModalConfirm` —
            // `confirm_top` synthesizes Enter, which `Modal::Message`
            // already closes on.
            Hit::ModalCancel => {
                let Some(res) = self.modals.cancel_top() else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::ModalConfirm => {
                let Some(res) = self.modals.confirm_top() else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::PaletteRow(i) => {
                let Some(Modal::Palette(state)) = self.modals.top_mut() else {
                    return false;
                };
                // Single click runs the command (spec §6) — no
                // select-then-confirm step for the palette.
                state.select(i);
                let Some(res) = state.confirm() else {
                    return false;
                };
                self.apply_modal_result(res)
            }
            Hit::ChooserToggle => {
                let Some(Modal::Chooser(state)) = self.modals.top_mut() else {
                    return false;
                };
                let Some(action) = state.toggle_action().cloned() else {
                    return false;
                };
                self.update(action)
            }
            step @ (Hit::ChooserStepDown | Hit::ChooserStepUp) => {
                let Some(Modal::Chooser(state)) = self.modals.top() else {
                    return false;
                };
                let up = step == Hit::ChooserStepUp;
                let Some(action) = state.step_action(up).cloned() else {
                    return false;
                };
                self.update(action)
            }
            Hit::PickerRow(i) => {
                let Some(Modal::FilePicker(state)) = self.modals.top_mut() else {
                    return false;
                };
                // A click is a choice even when the row was already lit
                // (row 0 opens selected): `select` records it, so Enter
                // semantics act on the row rather than the save prefill.
                let was_selected = state.selected() == i;
                state.select(i);
                if was_selected || clicks.list == 2 {
                    let Some(res) = state.activate() else {
                        return self.update(Action::Render);
                    };
                    self.apply_modal_result(res)
                } else {
                    self.update(Action::Render)
                }
            }
            Hit::PickerPrimary => {
                let Some(Modal::FilePicker(state)) = self.modals.top_mut() else {
                    return false;
                };
                let Some(res) = state.confirm_here() else {
                    return self.update(Action::Render);
                };
                self.apply_modal_result(res)
            }
            Hit::PickerHidden => {
                let Some(Modal::FilePicker(state)) = self.modals.top_mut() else {
                    return false;
                };
                state.toggle_hidden();
                self.update(Action::Render)
            }
            Hit::NewProjectBrowse => self.update(Action::BrowseNewProjectDir),
            // A disabled row registers no hit, so `i` is always a row
            // `select` takes: the hit map is the last frame's, and every
            // event that swaps a chooser's rows redraws before the next
            // event is read (task #18).
            Hit::ChooserRow(i) => {
                let Some(Modal::Chooser(state)) = self.modals.top_mut() else {
                    return false;
                };
                if state.selected() == i || clicks.list == 2 {
                    let Some(res) = state.confirm() else {
                        return false;
                    };
                    self.apply_modal_result(res)
                } else {
                    state.select(i);
                    self.sync_theme_preview();
                    self.update(Action::Render)
                }
            }
            Hit::VarPickerRow(i) => {
                let Some(Modal::VarPicker(state)) = self.modals.top_mut() else {
                    return false;
                };
                if state.selected() == i || clicks.list == 2 {
                    let Some(res) = state.confirm() else {
                        return false;
                    };
                    self.apply_modal_result(res)
                } else {
                    state.select(i);
                    self.update(Action::Render)
                }
            }
            Hit::ConfigStartupChoice(choice) => self.update(Action::ConfigStartupChoice(choice)),
            Hit::KeymapChoice(answer) => self.update(Action::KeymapAnswer(answer)),
            Hit::ConfigEditKeepEditing => {
                let Some(Modal::ConfigEditInvalid { file, path, .. }) = self.modals.top() else {
                    return false;
                };
                let (file, path) = (*file, path.clone());
                self.keep_editing_config_edit(file, path)
            }
            Hit::ConfigEditDiscard => {
                let Some(Modal::ConfigEditInvalid { path, .. }) = self.modals.top() else {
                    return false;
                };
                let path = path.clone();
                self.update(Action::ConfigEditDiscard { path })
            }
            Hit::ConfirmChoice(c) => {
                let Some(Modal::Confirm { choices, .. }) = self.modals.top() else {
                    return false;
                };
                let Some((_, _, actions)) = choices.iter().find(|(choice, _, _)| *choice == c)
                else {
                    return false;
                };
                let res =
                    ModalResult { actions: actions.clone(), close: true, ..Default::default() };
                self.apply_modal_result(res)
            }
            Hit::ResponseTab(mode) => {
                self.update(Action::FocusPane(PaneId::Response));
                self.update(Action::ResponseViewMode(mode))
            }
            Hit::JsonRow(i) => {
                self.update(Action::FocusPane(PaneId::Response));
                // The click also anchors a possible selection sweep (and
                // collapses any previous selection), like any text click.
                if self.response_click_selection(clicks.text, m) {
                    self.text_drag = Some(TextDrag::Response);
                }
                self.update(Action::JsonRowClicked { row: i, toggle: false })
            }
            Hit::JsonArrow(i) => {
                self.update(Action::FocusPane(PaneId::Response));
                self.update(Action::JsonRowClicked { row: i, toggle: true })
            }
            // As `/` does: the profile's syntax, forward (amendment A5.2).
            Hit::ResponseSearchButton => {
                self.update(Action::OpenResponseSearch { backward: false })
            }
            Hit::ResponseSearchNext => self.step_response_search(1),
            Hit::ResponseSearchPrev => self.step_response_search(-1),
            Hit::CopyUrl => {
                self.update(Action::FocusPane(PaneId::Editor));
                self.update(Action::CopyToClipboard(CopyTarget::Url))
            }
            Hit::CopyBodyButton => self.update(Action::CopyToClipboard(CopyTarget::ResponseView)),
            Hit::SaveBodyButton => self.update(Action::PromptSaveView),
            Hit::ResponseEditorButton => self.update(Action::OpenResponseInEditor),
            Hit::ResponseJqButton => self.update(Action::ToggleJqBar),
            Hit::ResponseJqBar => {
                let was_focused = self.session.response.jq_focused();
                self.update(Action::FocusPane(PaneId::Response));
                if !self.session.response.set_jq_focus(true) {
                    return false;
                }
                // The bar's rect is the text window, past the `jq ` label
                // (a press on the label maps to the window's first column),
                // drawn from 0 unfocused and following the caret once
                // focused.
                self.press_text(FieldId::Jq, m, clicks, was_focused);
                self.update(Action::Render)
            }
            Hit::ResponseJqAiButton => self.update(Action::OpenJqDescribe),
            Hit::ResponseJqChip => self.update(Action::OpenJqViews),
            Hit::ResponseJqRevert => self.update(Action::RevertJqView),
            // The bar's ✕ is a press on the bar, like a search box's: the
            // filter clears and the bar takes the caret, as a click on its
            // text gives it, so the emptied bar stays up to type into. A bar
            // the pointer can reach is drawn, so it can take the caret
            // (amendments B8, C4).
            Hit::ResponseJqClear => {
                // A refused clear (an open input that would not close, #75,
                // or nothing left to clear) moves the caret nowhere.
                self.last_action_failed = false;
                let cleared = self.update(Action::ClearJqFilter);
                if self.last_action_failed {
                    return cleared;
                }
                self.update(Action::FocusPane(PaneId::Response));
                self.session.response.set_jq_focus(true);
                self.update(Action::Render)
            }
            Hit::HeaderCopy(i) => self.update(Action::CopyToClipboard(if alt_click(&m) {
                CopyTarget::ResponseHeaderLine(i)
            } else {
                CopyTarget::ResponseHeader(i)
            })),
            Hit::AutoHeaderCopy(i) => self.update(Action::CopyToClipboard(if alt_click(&m) {
                CopyTarget::ComputedHeaderLine(i)
            } else {
                CopyTarget::ComputedHeader(i)
            })),
            Hit::AutoHeaderOverride(i) => self.update(Action::OverrideAutoHeader(i)),
            Hit::AutoHeaderReveal => self.update(Action::ToggleHeaderReveal),
            Hit::ScrollbarThumb(pane) => {
                let Some(thumb) = self.hits.rect_of(&Hit::ScrollbarThumb(pane)) else {
                    return false;
                };
                self.drag = Some(Drag {
                    pane,
                    grab_offset: m.row.saturating_sub(thumb.y),
                    horizontal: false,
                });
                // Redraw so the thumb picks up its dragged styling.
                self.update(Action::Render)
            }
            Hit::ScrollbarTrack(pane, delta) => {
                self.update(Action::ScrollPane(pane, delta.clamp(-30, 30)))
            }
            Hit::HScrollThumb(pane) => {
                let Some(thumb) = self.hits.rect_of(&Hit::HScrollThumb(pane)) else {
                    return false;
                };
                self.drag = Some(Drag {
                    pane,
                    grab_offset: m.column.saturating_sub(thumb.x),
                    horizontal: true,
                });
                self.update(Action::Render)
            }
            // Only the Response pane draws a horizontal bar today; a track
            // click pages the viewport a full width toward the click.
            Hit::HScrollTrack(PaneId::Response, delta) => {
                self.session.response.handle_scroll_h(delta);
                self.update(Action::Render)
            }
            Hit::HScrollTrack(..) => false,
            // A drawn `{{token}}` is never a click target (`Down(Left)`
            // resolves past it); the arm keeps the match exhaustive.
            Hit::VarToken(_) => false,
            // A surface of its own, as a table row is (its hover is what
            // the footer reads, task #59). Its click focuses the editor,
            // as a click anywhere in the auto section always has.
            Hit::AutoHeaderRow(_) => self.update(Action::FocusPane(PaneId::Editor)),
            // The tooltip's controls, which their keys share (#135).
            Hit::TipEdit(name) => self.press_tip_pill(crate::action::TipPill::Edit, name),
            Hit::TipCopy(name) => self.press_tip_pill(crate::action::TipPill::Copy, name),
            Hit::TipReveal(name) => self.press_tip_pill(crate::action::TipPill::Reveal, name),
            // A click on the panel between its controls: consumed, so it
            // never reaches whatever the tip floats over.
            Hit::TipPanel(_) => true,
            // The input closed at the top of this function (a close it
            // refused swallowed the click there), so selecting a row never
            // resets `form`/`grid` over live typing.
            Hit::VmLeftRow(i) => {
                self.varmanager.select_row(i);
                self.update(Action::Render)
            }
            // The Manager's environment switcher is the header env chip's
            // chooser, reached from the screen that replaced the header.
            Hit::VmNewVar => self.update(Action::PromptNewVar),
            Hit::VmNewSelector => self.update(Action::PromptNewSelector),
            // A second click on the field already under edit is inert (the
            // top-of-`on_hit` close left it alone precisely so this check
            // still sees the live edit). A click on any other field lands
            // here only once that close saved whatever was being edited (a
            // close it refused swallowed the click there), so this starts
            // a fresh edit on the clicked field.
            Hit::VmFormField(field) => {
                let already_editing =
                    self.varmanager.form.editing.as_ref().is_some_and(|e| e.field == field);
                if !already_editing {
                    let Self { project, varmanager, .. } = self;
                    if let Some(p) = project {
                        varmanager.start_field_edit(p, field);
                    }
                }
                // Through the field's one-row `Well` (the drawn window
                // starts at 0 unless the field was already under edit).
                self.press_text(FieldId::VmForm(field), m, clicks, already_editing);
                self.update(Action::Render)
            }
            // The grid's cells follow `VmFormField`'s rules exactly: a
            // second click on the live cell is inert, and a click on
            // another cell lands only once the close saved the first.
            Hit::VmEntryCell { row, col } => {
                let already_editing = self
                    .varmanager
                    .grid
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.row == row && e.col == col);
                if !already_editing {
                    self.varmanager.grid.cursor = (row, col);
                    self.vm_start_cell_edit(row, col);
                }
                // The cell draws windowed at the hit rect's own left edge;
                // the resting text draws from 0. A ghost-row click gets
                // redirected to the empty name cell by `start_cell_edit`,
                // which then isn't this cell, so nothing is placed.
                self.press_text(FieldId::VmGrid { row, col }, m, clicks, already_editing);
                self.update(Action::Render)
            }
            Hit::VmEntryRadio(row) => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let (Some(env), Some(option)) = (
                    self.active_env().map(str::to_string),
                    self.project().and_then(|p| self.varmanager.entry_at(p, row)),
                ) else {
                    return false;
                };
                self.varmanager.grid.cursor = (row, 0);
                self.varmanager.focus = VmFocus::Grid;
                self.update(Action::VarEdit(VarEditOp::SelectOption { env, selector, option }))
            }
            Hit::VmEntryCopy(row) => {
                // The click takes the grid the way the trash beside it
                // does, so the keyboard is left where the pointer acted.
                self.varmanager.grid.cursor = (row, 0);
                self.varmanager.focus = VmFocus::Grid;
                self.update(Action::CopyOption { row })
            }
            Hit::VmPasteOption => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let Some(env) = self
                    .project()
                    .and_then(|p| crate::components::varmanager::op_env(p, &selector))
                else {
                    return false;
                };
                self.update(Action::VarStruct(VarStructOp::PasteOption { env, selector, count: 1 }))
            }
            Hit::VmEntryDelete(row) => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let (Some(env), Some(name)) = (
                    self.active_env().map(str::to_string),
                    self.project().and_then(|p| self.varmanager.entry_at(p, row)),
                ) else {
                    return false;
                };
                self.varmanager.grid.cursor = (row, 0);
                self.varmanager.focus = VmFocus::Grid;
                self.update(Action::DeleteEntry { env, selector, name })
            }
            Hit::VmNewOption => {
                self.update(Action::StartNewOptionEdit);
                self.update(Action::Render)
            }
            Hit::VmEditFields => {
                let crate::components::varmanager::VmDetail::Group(selector) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::PromptGroupFields { selector })
            }
            Hit::VmSecretToggle => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::ToggleSecretVar { name })
            }
            Hit::VmRevealToggle => self.update(Action::ToggleVarReveal),
            Hit::VmRemoveEnvValue => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                self.update(Action::RemoveVarValue {
                    name,
                    destination: crate::action::ExtractDestination::ActiveEnv,
                })
            }
            // Both buttons are on the variable form *and* the selector pane;
            // the rename/delete flows behind them already branch on what
            // the name is declared as.
            Hit::VmRename => {
                let Some(name) = self.varmanager.detail.name().map(str::to_string) else {
                    return false;
                };
                self.update(Action::PromptRenameVar { from: name })
            }
            Hit::VmDelete => {
                let Some(name) = self.varmanager.detail.name().map(str::to_string) else {
                    return false;
                };
                self.update(Action::DeleteVar { name })
            }
            Hit::VmPromoteBtn => {
                let crate::components::varmanager::VmDetail::Var(name) =
                    self.varmanager.detail.clone()
                else {
                    return false;
                };
                let open_request =
                    self.editor.slug.is_some().then(|| self.editor.current_request());
                let Some((_, action)) = self.project().and_then(|p| {
                    crate::components::varmanager::promote_action(p, open_request.as_ref(), &name)
                }) else {
                    return false;
                };
                self.update(action)
            }
        }
    }
}
