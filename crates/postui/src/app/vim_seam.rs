//! The vim profile's text fields (piece 4 spec §3.3, §3.7; plan M3a): the
//! session edges, keys into the engine, and what the engine hands back.
//! Every method here is `impl App`. The engine itself is reached only
//! through `keyroute::engine_glue`.

use super::router::Ran;
use super::*;
use crate::config::KeyProfile;
use crate::keyroute::engine_glue::{
    self, AppRequest, Edge, FieldBuf, FieldSession, Glue, Mode, Mouse, Note, OpenAs, OpenSeat,
    Outcome, Pos, Settled, Shape, Start, ViewCtx,
};
use crate::keyroute::notation::Tok;
use crate::keyroute::table::{self, Intent, Match};
use crate::keyroute::{EngineKind, FieldCaret, FieldId, Step};

/// Everything the App keeps for the vim profile. Only this module reads or
/// writes it (the boundary test in `lib.rs` holds the rest of the crate to
/// that); the rest of the App reaches vim through this module's `vim_*`
/// methods.
#[derive(Default)]
pub(crate) struct VimState {
    /// The vim profile's text engine and its field session (spec §3.7):
    /// the one door to the engine (`keyroute::engine_glue`).
    pub(crate) glue: Glue,
    /// The top dialog's field's engine (plan M3b Deviation 1): a second
    /// engine, so a screen field parked under a dialog keeps its state.
    pub(crate) modal_glue: Glue,
    /// Vim's one last search pattern and its `'hlsearch'` state (plan M5b
    /// Deviation 20, the user's review ruling: the app owns it). It lives
    /// as long as the app, across request and project switches, which swap
    /// the response; the response and each engine have it lent for the
    /// call that needs it.
    pub(crate) search_memory: crate::vim::engine::SearchMemory,
    /// The dialog field with a session, by its dialog's serial (plan M3b
    /// Deviation 2).
    pub(crate) modal_session: Option<(u64, FieldId)>,
    /// The top dialog's serial at the last sync: a dialog field that opens
    /// under a new top opens in Insert (plan M3b Deviation 3).
    pub(crate) modal_seen: Option<u64>,
    /// The `:` line's history (plan M5b Task 5): read from `ui.toml` at
    /// startup, written back at quit.
    pub(crate) cmdline_history: crate::cmdline::CmdlineHistory,
    /// An add verb ran during this event (`OnSelection(Add | AddAlt)`):
    /// what it opened opens in Insert (spec §5.6), whether the verb came
    /// from its row, the `:` line, or a click on the line's pinned row. The
    /// next `sync_session` consumes it, so a key and a click read it alike.
    add_opens_insert: bool,
    /// `.` (spec §5.12; plan M5b Deviations 5–8): the last list change a
    /// vim key made, while no other change has been made since.
    dot: Option<Dot>,
    /// What this event did to `.`, settled once the event loop has
    /// captured its undo step (`vim_settle_dot`).
    dot_pending: DotPending,
    /// Vim's `:` opened the line over the response's V: the selection ends
    /// once every dialog is closed, as Vim's Visual has ended when its
    /// `:'<,'>` line does (`end_visual_off_response`).
    ex_over_visual: bool,
    /// What the mouse did on a field's text during this event, which the
    /// engine settles at its end (`vim_mouse_end`).
    text_mouse: Option<(FieldId, Pos, Mouse)>,
    /// This event's left press landed on a window separator (the sidebar
    /// gutter, or the response's header strip), which `vim_mouse_end` does
    /// not settle as a click.
    separator_press: bool,
}

/// The change `.` repeats (spec §5.12) and the history's change count
/// right after it: any change since makes it stale (plan M5b Deviation 6).
#[derive(Debug, Clone, Copy)]
struct Dot {
    change: crate::keyroute::Change,
    at: u64,
}

/// What a key or mouse event did to `.`, settled once the event loop has
/// captured the event's undo step (plan M5b Deviation 8).
#[derive(Debug, Clone, Copy, Default)]
enum DotPending {
    #[default]
    None,
    /// A list change ran, from the history's change count `before`.
    Made { change: crate::keyroute::Change, before: u64 },
    /// The palette ran what it ran from `before`, with no field open under
    /// it: that leaves `.` alone, as Vim's command-line commands do.
    Kept { before: u64 },
}

/// What an event was, for the session's settle (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    /// The start of an event: catch up with what changed since the last.
    Before,
    Key {
        routed: bool,
    },
    /// A mouse button event, and how the engine settles after it.
    Mouse(Option<Settled>),
}

/// The footer's word for the engine's mode (spec §3.8; plan M3a Deviation 14).
pub(crate) fn mode_word(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Replace => "REPLACE",
        Mode::InsertNormal { .. } => "(insert)",
        Mode::Visual(Shape::Char) => "VISUAL",
        Mode::Visual(Shape::Line) => "V-LINE",
        Mode::Search(_) => "SEARCH",
    }
}

impl App {
    /// The glue `id` runs on and `id`'s buffer at once (disjoint fields),
    /// or no buffer when that field is not open. A dialog's field lends the
    /// dialog engine and the top dialog's input (plan M3b Deviation 1).
    pub(crate) fn session_parts(&mut self, id: FieldId) -> (&mut Glue, Option<FieldBuf<'_>>) {
        // The body's 'shiftwidth' follows its indent (spec 2026-10-15 R11),
        // which the editor holds ready: no scan here.
        self.vim.glue.set_body_indent(self.editor.body_indent());
        match id {
            FieldId::Body => (
                &mut self.vim.glue,
                Some(FieldBuf::Body(&mut self.editor.body, &mut self.editor.body_visual)),
            ),
            FieldId::ModalInput(i) => {
                (&mut self.vim.modal_glue, self.modals.text_input_mut(i).map(FieldBuf::Line))
            }
            FieldId::Palette
            | FieldId::ChooserFilter
            | FieldId::VarPickerFilter
            | FieldId::FilePicker => {
                (&mut self.vim.modal_glue, self.modals.query_input_mut().map(FieldBuf::Line))
            }
            FieldId::Jq => (
                &mut self.vim.glue,
                Some(FieldBuf::Line(&mut self.session.response.jq_bar_mut().input)),
            ),
            FieldId::Search => {
                (&mut self.vim.glue, self.session.response.search_input_mut().map(FieldBuf::Line))
            }
            _ => (
                &mut self.vim.glue,
                line_buf(
                    id,
                    &mut self.editor.url,
                    &mut self.editor.table,
                    &mut self.varmanager,
                    &mut self.settings,
                )
                .map(FieldBuf::Line),
            ),
        }
    }

    /// The glue, the body and `to`'s buffer at once, for a carry out of the
    /// body (amendment C3).
    fn body_and(&mut self, to: FieldId) -> (&mut Glue, FieldBuf<'_>, Option<FieldBuf<'_>>) {
        let body = FieldBuf::Body(&mut self.editor.body, &mut self.editor.body_visual);
        let to = line_buf(
            to,
            &mut self.editor.url,
            &mut self.editor.table,
            &mut self.varmanager,
            &mut self.settings,
        )
        .map(FieldBuf::Line);
        (&mut self.vim.glue, body, to)
    }

    /// The field with an engine session now, and whether it has the caret
    /// (spec §3.3). A modal over an engine field parks it, whether the modal
    /// has a field of its own or not. Only in the vim profile.
    pub(crate) fn engine_field(&self) -> Option<(FieldId, bool)> {
        if self.profile() != KeyProfile::Vim {
            return None;
        }
        match self.field_caret() {
            FieldCaret::Live(id) if !id.is_modal() => Some((id, true)),
            FieldCaret::Live(_) => self.screen_field().map(|id| (id, false)),
            FieldCaret::Parked(id) => Some((id, false)),
            FieldCaret::None => None,
        }
    }

    /// Brings the engine session in line with where the caret is (spec
    /// §3.3): open, carry, close, keep or rebase, then settle after a click
    /// or a key the router ran itself. Runs at the start and the end of
    /// every key and mouse button event (plan M3a Deviation 2).
    pub(crate) fn sync_session(&mut self, open_as: Option<OpenAs>, cause: Cause) {
        // An add verb's row, its `:` verb and a click on that verb's pinned
        // row open what they make alike, in Insert (spec §5.6; the final
        // review's I-3). Taken on every sync, so it never outlives the
        // event that set it.
        let add = std::mem::take(&mut self.vim.add_opens_insert);
        let open_as = open_as.or(add.then_some(OpenAs::Insert(OpenSeat::Keep)));
        let profile = self.profile();
        let was = self.vim.glue.profile();
        let switched_in = profile == KeyProfile::Vim && was == Some(KeyProfile::Arrows);
        self.vim.glue.set_profile(profile);
        if profile != KeyProfile::Vim {
            // Arrows has no Normal mode: an open field goes on as main's
            // modeless field, its text and caret untouched (spec §4.3).
            // Leaving Insert would step the caret back off the end, so it
            // is put back where it was.
            if let Some(id) = self.vim.glue.owned() {
                let caret = self.session_parts(id).1.map(|buf| buf.caret());
                self.close_session(id);
                if let Some(at) = caret {
                    self.put_caret(id, at);
                }
                self.unpaint(id, true);
                self.vim.glue.set_session(FieldSession::None);
            }
            // The dialog session goes the same way, and its engine gives
            // the registers back (plan M3b Deviation 1). A dialog no longer
            // on top is left through the ghost, as `sync_modal_session`
            // leaves it: its field index would name the top's field.
            if let Some((dialog, id)) = self.vim.modal_session.take() {
                if Some(dialog) == self.modals.top_serial() {
                    let caret = self.session_parts(id).1.map(|buf| buf.caret());
                    self.close_session(id);
                    if let Some(at) = caret {
                        self.put_caret(id, at);
                    }
                    self.unpaint(id, true);
                } else {
                    self.vim.modal_glue.leave_ghost();
                }
                self.vim.glue.swap_registers(&mut self.vim.modal_glue);
                self.vim.modal_glue.set_session(FieldSession::None);
            }
            // A body session that closed before the switch left edtui in
            // Normal, where the arrows body binds no keys: give it its
            // Insert back whether or not a session is still owned.
            if was == Some(KeyProfile::Vim) {
                self.unpaint(FieldId::Body, true);
                // The sidebar's row a `zc` hid is the vim cursor's; arrows'
                // `→` and `←` on the folder row act on the folder.
                self.sidebar.forget_fold_cursor();
            }
            return;
        }
        // Arrows to vim leaves an untouched auto-indent (spec 2026-10-16
        // R3): its blanks go before the engine takes the body, or its
        // Insert would open on text the request never held.
        if switched_in {
            self.editor.settle_auto_indent(false);
        }
        self.vim.glue.own_body(self.editor.slug.as_deref(), &mut self.vim.search_memory);
        let now = self.engine_field();
        let mouse = matches!(cause, Cause::Mouse(_));
        let edge = engine_glue::edge(self.vim.glue.owned(), now.map(|(id, _)| id));
        match edge {
            Edge::Idle => {}
            Edge::Open(id) => {
                // Arrows to vim with a field open: it was being typed in, so
                // it goes on in Insert (spec §4.3).
                let open = if switched_in { Some(OpenAs::Insert(OpenSeat::Keep)) } else { open_as };
                self.open_session(id, open, !mouse, Start::Normal);
            }
            // A carry across Insert-only is a close and an open (plan M3b
            // Deviation 16): the engine would keep Insert-only on the field
            // it carries to. So is an add verb's (plan M5b Deviation 19):
            // `:new` from an open cell opens the new row in Insert, as from
            // a closed table, where a carry would keep the cell's Normal.
            Edge::Carry(from, to) if add || from.insert_only() != to.insert_only() => {
                self.close_session(from);
                self.open_session(to, open_as, !mouse, Start::Normal);
            }
            Edge::Carry(from, to) => self.carry_session(from, to),
            // An add verb's row can land on the id that was open: #20's
            // pencil puts its row at the ghost row's index, and the table's
            // add row reopens an emptied ghost key cell, so from an open
            // ghost cell each opens the same cell id. What sets `add` makes
            // a new field every time (a table's add resolves to the ghost
            // row, or to nothing on the Body tab, which sets no flag), so
            // the id is a new row's and it opens fresh, in Insert, as it
            // would from any other cell (`:new` from an open cell, M5b).
            Edge::Same(id) if add => {
                self.close_session(id);
                self.open_session(id, open_as, !mouse, Start::Normal);
            }
            // Spec §3.3: the close edge also drops the router's pending
            // keys (amendment A4-1), but not the recording flag (the final
            // review's ruling I-4).
            Edge::Close(id) => {
                self.close_session(id);
                self.router.clear_pending();
            }
            Edge::Same(id) => {
                let (glue, buf) = self.session_parts(id);
                if let Some(mut buf) = buf
                    && glue.differs(&buf)
                {
                    glue.rebase(&mut buf);
                }
            }
        }
        self.vim.glue.set_session(match now {
            Some((id, true)) => FieldSession::Live(id),
            Some((id, false)) => FieldSession::Parked(id),
            None => FieldSession::None,
        });
        let how = match cause {
            Cause::Mouse(how) => how,
            Cause::Key { routed: true } if matches!(edge, Edge::Same(_)) => Some(Settled::Key),
            _ => None,
        };
        if let (Some(how), Some(id)) = (how, self.vim.glue.live()) {
            let (glue, buf) = self.session_parts(id);
            if let Some(mut buf) = buf {
                glue.settle(&mut buf, how);
            }
        }
        self.sync_modal_session(open_as, cause, switched_in);
    }

    /// The dialog session's half of [`Self::sync_session`] (plan M3b
    /// Deviations 1–3): open, carry within one dialog, move to another
    /// dialog, close, or rebase. The registers go to the dialog engine when
    /// a dialog session opens and come back when it closes; the last search
    /// pattern is the one both engines borrow (plan M5b Deviation 20).
    fn sync_modal_session(&mut self, open_as: Option<OpenAs>, cause: Cause, switched_in: bool) {
        let now = self.modal_engine_field();
        let was = self.vim.modal_session;
        let top = self.modals.top_serial();
        let fresh = top != self.vim.modal_seen;
        self.vim.modal_seen = top;
        let mouse = matches!(cause, Cause::Mouse(_));
        // A field opens in Insert when its dialog is new, on a click, after
        // a dialog over it closed, and when vim was just switched on; from
        // the keyboard in a dialog that stays, in Normal (Deviation 3).
        let default = if fresh || mouse || switched_in { Start::Insert } else { Start::Normal };
        match (was, now) {
            (None, None) => {}
            // A field reseeded in place (`alt+d`'s fresh `LineInput`) has
            // lost its mark: the close step starts from the new text, as
            // arrows' first snapshot does (Deviation 11). A `set_text`
            // reseed kept the first mark, and keeps it.
            (Some(a), Some(b)) if a == b => {
                let (glue, buf) = self.session_parts(b.1);
                if let Some(mut buf) = buf
                    && glue.differs(&buf)
                {
                    glue.rebase(&mut buf);
                    if let FieldBuf::Line(input) = &mut buf {
                        input.mark_open();
                    }
                }
            }
            // Tab in a form: the same dialog, the mode carries (spec §3.3).
            (Some((from_dialog, from)), Some((to_dialog, to))) if from_dialog == to_dialog => {
                self.carry_session(from, to)
            }
            // Another dialog took the caret, or came back to it. The field
            // left belongs to a dialog that is not the top, so it is left
            // through the ghost: its index would name the new top's field.
            (Some(_), Some((_, to))) => {
                self.vim.modal_glue.leave_ghost();
                self.open_session(to, open_as, !mouse, default);
            }
            (None, Some((_, to))) => {
                self.vim.glue.swap_registers(&mut self.vim.modal_glue);
                self.open_session(to, open_as, !mouse, default);
            }
            // The field closed in its own dialog (`j` to the buttons), or
            // its dialog was popped or covered: then it is left through the
            // ghost, as above, since its index would name the top's field.
            (Some((from_dialog, from)), None) => {
                if Some(from_dialog) == top {
                    self.close_session(from);
                } else {
                    self.vim.modal_glue.leave_ghost();
                }
                self.vim.glue.swap_registers(&mut self.vim.modal_glue);
                self.router.clear_pending();
            }
        }
        self.vim.modal_session = now;
        self.vim
            .modal_glue
            .set_session(now.map_or(FieldSession::None, |(_, id)| FieldSession::Live(id)));
        let how = match cause {
            Cause::Mouse(how) => how,
            Cause::Key { routed: true } if was == now => Some(Settled::Key),
            _ => None,
        };
        if let (Some(how), Some((_, id))) = (how, now) {
            let (glue, buf) = self.session_parts(id);
            if let Some(mut buf) = buf {
                glue.settle(&mut buf, how);
            }
        }
    }

    /// The dialog field with the caret, and its dialog (plan M3b Deviation
    /// 2). Only in the vim profile.
    fn modal_engine_field(&self) -> Option<(u64, FieldId)> {
        if self.profile() != KeyProfile::Vim {
            return None;
        }
        match self.field_caret() {
            FieldCaret::Live(id) if id.is_modal() => self.modals.top_serial().map(|s| (s, id)),
            _ => None,
        }
    }

    /// `id` got the caret: the engine enters it in the mode the key asked
    /// for, else `default`: Normal on the screen; a dialog field's rule is
    /// plan M3b Deviation 3; a query box is always Insert-only (spec §5.6;
    /// plan M3a Deviation 12). A keyboard open drops a GUI selection (the
    /// Settings field selects all for arrows). A press that opened a
    /// one-line field keeps the anchor it planted, which seating the caret
    /// drops, so a drag that follows selects in a query box, whose GUI
    /// selection stands (final review I-3). Elsewhere the press settles
    /// after the open, and the engine selects from it.
    fn open_session(&mut self, id: FieldId, open: Option<OpenAs>, keyboard: bool, default: Start) {
        let (glue, buf) = self.session_parts(id);
        let Some(mut buf) = buf else { return };
        let mut pressed = false;
        if let FieldBuf::Line(input) = &mut buf {
            if keyboard {
                input.clear_selection();
            }
            pressed = !keyboard && input.click_anchored();
        }
        let (start, seat) = match open {
            _ if id.insert_only() => (Start::InsertOnly, OpenSeat::Keep),
            Some(OpenAs::Insert(seat)) => (Start::Insert, seat),
            Some(OpenAs::Normal) => (Start::Normal, OpenSeat::Keep),
            None => (default, OpenSeat::Keep),
        };
        glue.enter(&mut buf, start, seat);
        if let FieldBuf::Line(input) = &mut buf {
            if pressed {
                input.begin_mouse_selection();
            }
            // A dialog records its field's close from the text at open
            // (plan M3b Deviation 11).
            if id.is_modal() {
                input.mark_open();
            }
        }
    }

    /// The caret moved from `from` straight to `to` (spec §3.3's carry). The
    /// body still exists, so the engine leaves the real one (amendment C3):
    /// an unused autoindent goes from its text, and its history still fits
    /// that text at the next visit. Every other source is left through the
    /// glue's ghost, its stale paint cleared (plan M3a Deviation 4): a
    /// one-line source, whether gone (a cell the key committed) or not, and
    /// a body whose text was replaced since the engine last saw it, as
    /// `close_session` treats it.
    fn carry_session(&mut self, from: FieldId, to: FieldId) {
        if from == FieldId::Body {
            let (glue, mut body, to) = self.body_and(to);
            if !glue.differs(&body) {
                match to {
                    Some(mut to) => glue.carry_from(&mut body, &mut to),
                    None => glue.leave(&mut body),
                }
                return;
            }
        }
        self.unpaint(from, false);
        let (glue, buf) = self.session_parts(to);
        match buf {
            Some(mut buf) => {
                glue.carry(&mut buf);
                if let (true, FieldBuf::Line(input)) = (to.is_modal(), &mut buf) {
                    input.mark_open();
                }
            }
            None => glue.leave_ghost(),
        }
    }

    /// `id` lost the caret. Left in place while its buffer is still the one
    /// the engine knows, else through the glue's ghost, with the stale paint
    /// cleared (plan M3a Deviation 4).
    fn close_session(&mut self, id: FieldId) {
        let gone = {
            let (glue, buf) = self.session_parts(id);
            match buf {
                Some(mut buf) if !glue.differs(&buf) => {
                    glue.leave(&mut buf);
                    false
                }
                _ => {
                    glue.leave_ghost();
                    true
                }
            }
        };
        if gone {
            self.unpaint(id, false);
        }
    }

    /// Clears the engine's paint from `id`'s buffer if it still exists: the
    /// Visual span, and for the body edtui's mode (`modeless` gives edtui
    /// its Insert back, for the arrows profile).
    fn unpaint(&mut self, id: FieldId, modeless: bool) {
        match self.session_parts(id).1 {
            Some(FieldBuf::Line(input)) => input.set_visual(None),
            Some(FieldBuf::Body(state, visual)) => {
                *visual = None;
                state.mode =
                    if modeless { edtui::EditorMode::Insert } else { edtui::EditorMode::Normal };
            }
            None => {}
        }
    }

    /// Puts `id`'s caret back at `at`, clamped into its text, if the field
    /// is still open.
    fn put_caret(&mut self, id: FieldId, at: Pos) {
        match self.session_parts(id).1 {
            Some(FieldBuf::Line(input)) => input.set_cursor(at.col),
            Some(FieldBuf::Body(state, _)) => {
                let row = at.row.min(state.lines.len().saturating_sub(1));
                let len = state.lines.get(edtui::RowIndex::new(row)).map_or(0, Vec::len);
                state.cursor = edtui::Index2::new(row, at.col.min(len));
            }
            None => {}
        }
    }

    /// The field whose session is live and that has the caret: the one
    /// the engine may be handed keys, pastes and copies for.
    pub(crate) fn live_engine_field(&self) -> Option<FieldId> {
        let caret = self.field_caret();
        [self.vim.modal_glue.live(), self.vim.glue.live()]
            .into_iter()
            .flatten()
            .find(|&id| caret == FieldCaret::Live(id))
    }

    /// Drops the half-typed keys, the router's and the live field's engine's,
    /// as Vim's ctrl+c cancels a pending command: the app's ctrl+c is taken
    /// before either sees it (task #85), so its first press, which only
    /// arms the quit, says so here. The recording flag stays, as for a
    /// field close.
    pub(crate) fn vim_drop_pending(&mut self) {
        self.router.clear_pending();
        match self.live_engine_field() {
            Some(id) if id.is_modal() => self.vim.modal_glue.clear_pending(),
            Some(_) => self.vim.glue.clear_pending(),
            None => {}
        }
    }

    /// The engine `id` runs on: the dialog engine for a dialog's field
    /// (plan M3b Deviation 1), else the screen's.
    pub(crate) fn glue_of(&self, id: FieldId) -> &Glue {
        if id.is_modal() { &self.vim.modal_glue } else { &self.vim.glue }
    }

    /// What the engine needs to know about the view (piece 3 §4.1): the
    /// body's rows as last drawn.
    pub(crate) fn view_ctx(&self, id: FieldId) -> ViewCtx {
        let rows = self.editor.last_body_area.map(|a| usize::from(a.height));
        ViewCtx { viewport_rows: if id == FieldId::Body { rows } else { None } }
    }

    /// One key into the engine on the live field (spec §3.7). `None` when
    /// that field closed under an earlier key of the same run: the rest of
    /// what was typed for it goes nowhere.
    pub(super) fn engine_key(&mut self, ev: KeyEvent) -> Option<Ran> {
        let id = self.live_engine_field()?;
        if self.box_claims(id, &ev) {
            return Some(Ran::routed(self.handle_key_routed(ev)));
        }
        // Read before the key: whether a `u` or ctrl+r it hands back means
        // the field had nothing to step (final review M-1).
        let idle = self.glue_of(id).undo_ready();
        let out = self.engine_handle(id, ev)?;
        Some(match out {
            Outcome::Consumed { changed, note, request } => {
                self.engine_feedback(note, request);
                // Any key the engine takes leaves the jq bar's entered menu
                // row, a move as well as an edit, as in arrows.
                if id == FieldId::Jq {
                    self.session.response.leave_jq_menu();
                }
                if changed {
                    self.query_edited(id);
                }
                self.after_typed(id, ev);
                Ran::consumed()
            }
            Outcome::Declined { count, keys } => self.route_declined(id, count, keys, idle),
        })
    }

    /// One key into `id`'s engine with the app's last pattern lent for it
    /// (plan M5b Deviation 20). The pattern is back on the app before this
    /// returns, also when `id`'s buffer is gone (`None`).
    fn engine_handle(&mut self, id: FieldId, ev: KeyEvent) -> Option<Outcome> {
        let view = self.view_ctx(id);
        let mut memory = std::mem::take(&mut self.vim.search_memory);
        let out = {
            let (glue, buf) = self.session_parts(id);
            buf.map(|mut buf| glue.handle(ev, &mut buf, &view, &mut memory))
        };
        self.vim.search_memory = memory;
        out
    }

    /// Keys a query box owns before the engine (plan M3b Deviation 5, the
    /// user's decision): the file picker's Backspace on an empty filter
    /// climbs a folder; the jq bar's Tab and shift+Tab are its completion's
    /// (menu mode rewrites a quoted key through the input's own keys, which
    /// a declined key's quiet input would mute: amendment R4), and its
    /// Right and End accept an offered completion.
    fn box_claims(&self, id: FieldId, ev: &KeyEvent) -> bool {
        let plain = ev.modifiers.is_empty();
        match id {
            FieldId::FilePicker => {
                plain
                    && ev.code == KeyCode::Backspace
                    && matches!(self.modals.top(), Some(Modal::FilePicker(p)) if p.input().text().is_empty())
            }
            FieldId::Jq => match ev.code {
                KeyCode::Tab | KeyCode::BackTab => {
                    !ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                }
                KeyCode::Right | KeyCode::End => {
                    plain && self.session.response.jq_accepts_completion()
                }
                _ => false,
            },
            // The ex line's Backspace on an empty query closes it (spec
            // §5.9, plan M4b Deviation 15).
            FieldId::Palette => {
                plain
                    && ev.code == KeyCode::Backspace
                    && matches!(self.modals.top(), Some(Modal::Palette(p)) if p.is_ex() && p.input().is_empty())
            }
            _ => false,
        }
    }

    /// The engine changed a query box's text: the box follows, as after a
    /// typed key (its rows, its filter). The theme picker has no query box
    /// (task #6), so no edit here moves its preview.
    fn query_edited(&mut self, id: FieldId) {
        match id {
            FieldId::Jq => {
                self.session.response.mark_jq_typed();
                self.sync_jq();
            }
            FieldId::ChooserFilter
            | FieldId::Palette
            | FieldId::VarPickerFilter
            | FieldId::FilePicker => self.modals.query_changed(),
            _ => {}
        }
    }

    /// The app's undo path (`step_history`) reaching a vim field: the
    /// field's own history first (spec §3.7). `None` when no engine field
    /// has the caret. `Some(false)` when it has nothing to step that way,
    /// or the engine is not in idle Normal, where the undo chords do
    /// nothing (plan M3a Deviation 8).
    pub(crate) fn vim_field_undo(&mut self, redo: bool) -> Option<bool> {
        let id = self.live_engine_field()?;
        let kind = id.engine_kind();
        let glue = self.glue_of(id);
        let can = if redo { glue.can_redo(kind) } else { glue.can_undo(kind) };
        let idle = glue.mode() == Mode::Normal && !glue.view().pending;
        if !can || !idle {
            return Some(false);
        }
        let key = if redo {
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
        } else {
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
        };
        if let Outcome::Consumed { note, request, .. } = self.engine_handle(id, key)? {
            self.engine_feedback(note, request);
        }
        Some(true)
    }

    /// The body's text replaced as one engine step while the body has an
    /// engine session, live or parked (spec §3.7: format, minify, clear).
    /// `false` leaves the rewrite to the caller.
    pub(crate) fn vim_replace_body(&mut self, text: &str) -> bool {
        if self.vim.glue.owned() != Some(FieldId::Body) {
            return false;
        }
        let (glue, buf) = self.session_parts(FieldId::Body);
        let Some(mut buf) = buf else { return false };
        glue.edit(&mut buf, &engine_glue::Edit::Whole(text));
        true
    }

    /// A paste into the live vim field (spec §3.7; plan M3a Deviation 13):
    /// Insert, Replace and the search prompt type it; in Visual (the
    /// mouse's too) it replaces the selection (the user's ruling, final
    /// review); anywhere else it goes in by Vim's bracketed-paste rule. Each
    /// is one engine step. A one-line field takes it flattened (today's
    /// rule). `None` when no engine field has the caret: today's routing
    /// pastes. A bracketed paste is an event of its own, so the session
    /// catches up first, as a key's does: a body replaced since (`$EDITOR`)
    /// rebases before the paste lands on it.
    pub(crate) fn vim_paste(&mut self, text: &str) -> Option<bool> {
        self.sync_session(None, Cause::Before);
        let id = self.live_engine_field()?;
        // Today's routing pastes into the file picker, its folder jump
        // included (plan M3b Deviation 13).
        if id == FieldId::FilePicker {
            return None;
        }
        let flat;
        let text = if id.engine_kind() == EngineKind::Line {
            flat = crate::components::line_input::flatten_paste(text);
            flat.as_str()
        } else {
            text
        };
        let feedback = {
            let (glue, buf) = self.session_parts(id);
            let mut buf = buf?;
            if let Some((from, to)) = glue.visual_span(&mut buf) {
                glue.edit(&mut buf, &engine_glue::Edit::Replace(from, to, text));
                None
            } else {
                match glue.paste(text, &mut buf) {
                    Outcome::Consumed { note, request, .. } => Some((note, request)),
                    Outcome::Declined { .. } => {
                        glue.edit(&mut buf, &engine_glue::Edit::Append(text));
                        None
                    }
                }
            }
        };
        if let Some((note, request)) = feedback {
            self.engine_feedback(note, request);
        }
        self.query_edited(id);
        Some(self.update(Action::Render))
    }

    /// A picked `{{token}}` into the field the picker was opened from
    /// (spec §3.7): at the caret in Insert, over the selection in Visual
    /// (as a paste), else by the bracketed-paste rule (plan M3a Deviation
    /// 13). Insert ctrl+o counts as Insert: the edit resumes it first, as
    /// `Glue::rebase` does. Only the fields main inserts into: the URL
    /// line, a cell, the body. `false` leaves it to today's path. A token
    /// into the body turns on the body's variables, as today's path does.
    /// A `whole` `{{token}}` that would land inside a token takes that
    /// token's place instead (#149), found at the caret the edit puts it at
    /// (after ctrl+o's resume). A Visual selection still wins.
    pub(crate) fn vim_insert(&mut self, text: &str, whole: bool) -> bool {
        let Some(id) = self.vim.glue.owned() else { return false };
        if !matches!(id, FieldId::Url | FieldId::TableCell { .. } | FieldId::Body) {
            return false;
        }
        let typing = matches!(
            self.vim.glue.mode(),
            Mode::Insert | Mode::Replace | Mode::InsertNormal { .. }
        );
        let (glue, buf) = self.session_parts(id);
        let Some(mut buf) = buf else { return false };
        let edit = if typing {
            if whole {
                engine_glue::Edit::AtCaretOverToken(text)
            } else {
                engine_glue::Edit::AtCaret(text)
            }
        } else if let Some((from, to)) = glue.visual_span(&mut buf) {
            engine_glue::Edit::Replace(from, to, text)
        } else if whole {
            engine_glue::Edit::AppendOverToken(text)
        } else {
            engine_glue::Edit::Append(text)
        };
        glue.edit(&mut buf, &edit);
        if id == FieldId::Body {
            self.turn_on_body_vars();
        }
        true
    }

    /// The live engine field's Visual text, which ctrl+c and cmd+c copy
    /// (the user's decision, final review I-2): a mouse selection in a vim
    /// field becomes Visual, so without it ctrl+c would quit.
    pub(crate) fn vim_selection_text(&mut self) -> Option<String> {
        let id = self.live_engine_field()?;
        let (glue, buf) = self.session_parts(id);
        glue.visual_text(&mut buf?)
    }

    /// An engine note is an Info toast; `"+y` copies through the app's
    /// clipboard path (plan M3a Deviation 15).
    pub(crate) fn engine_feedback(&mut self, note: Option<Note>, request: Option<AppRequest>) {
        if let Some(Note::Unsupported(text) | Note::Message(text)) = note {
            self.toasts.push(text, ToastKind::Info);
        }
        if let Some(AppRequest::CopyToClipboard(text)) = request {
            self.copy_text_with_toast(&text, "Copied to the clipboard".to_string());
        }
    }

    /// The engine's search prompt and highlight for the next draw (spec
    /// §5.6), on the rows the body shows. Nothing in arrows, or off the Body
    /// tab. The range reaches one screen either side of the viewport, so a
    /// jump that scrolls during the render (`G`, a far `n`) still paints its
    /// matches in that frame.
    pub(crate) fn vim_mirror_body_search(&mut self) {
        if self.profile() != KeyProfile::Vim || self.editor.active_tab != EditorTab::Body {
            self.editor.body_search = Default::default();
            return;
        }
        let top = self.editor.body.viewport_offset().1;
        let rows = match self.editor.last_body_area {
            Some(a) => {
                let h = usize::from(a.height);
                top.saturating_sub(h)..top.saturating_add(h.saturating_mul(2))
            }
            None => 0..usize::MAX,
        };
        let found = self.vim.glue.body_search(
            &mut self.editor.body,
            &mut self.editor.body_visual,
            rows,
            &mut self.vim.search_memory,
        );
        self.editor.body_search = found;
    }

    /// `{{` typed into the URL line or a cell opens the variable picker, as
    /// it does on main (`Editor::handle_key`).
    fn after_typed(&mut self, id: FieldId, ev: KeyEvent) {
        let typing = matches!(self.glue_of(id).mode(), Mode::Insert | Mode::Replace);
        if ev.code != KeyCode::Char('{')
            || !typing
            || !matches!(id, FieldId::Url | FieldId::TableCell { .. })
        {
            return;
        }
        let braces = matches!(self.session_parts(id).1, Some(FieldBuf::Line(input)) if input.ends_with_at_cursor("{{"));
        if braces {
            self.update(Action::OpenVarPicker { completing: true });
        }
    }

    /// A key the engine handed back (plan M3a Deviation 6). `idle`: the
    /// engine was in idle Normal before the key (`Glue::undo_ready`).
    fn route_declined(
        &mut self,
        id: FieldId,
        count: Option<usize>,
        keys: Vec<KeyEvent>,
        idle: bool,
    ) -> Ran {
        let n = count.unwrap_or(1);
        // The field had nothing to step: the app history's turn (spec §3.7).
        // Only from idle Normal: in Visual, or with an operator pending, the
        // engine hands `u` or ctrl+r back as a key it has no use for, and
        // Vim does nothing (final review M-1).
        match keys.last().and_then(Tok::of) {
            Some(Tok::Char('u')) if idle => {
                return self.run_steps(vec![Step::Action(Action::Undo); n]);
            }
            Some(Tok::Ctrl('r')) if idle => {
                return self.run_steps(vec![Step::Action(Action::Redo); n]);
            }
            _ => {}
        }
        let cap = self.repeat_cap(self.surface());
        // A field row the engine handed back whole (M4's `gt`).
        let toks: Option<Vec<Tok>> = keys.iter().map(Tok::of).collect();
        if let Some(toks) = toks
            && let Match::Complete(b) =
                table::lookup_class(&toks, KeyProfile::Vim, table::field_class(id, self.surface()))
        {
            return match b.intent {
                Intent::Swallow => Ran::default(),
                _ => self.run_steps(crate::keyroute::expand(b, count, cap)),
            };
        }
        let mut ran = Ran::default();
        for ev in keys {
            if !self.forwards(id, &ev) {
                continue;
            }
            // As a count before a named key does off a field (spec §3.4
            // step 4): only the moves repeat.
            let repeat = matches!(
                ev.code,
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
            );
            let times = if repeat { n.min(cap) } else { 1 };
            for _ in 0..times {
                ran = ran.and(self.route_forwarded(id, ev));
            }
        }
        ran
    }

    /// A declined key [`Self::forwards`] let through. A dialog or a query
    /// box takes it on its own arms with its text input quiet (plan M3b
    /// Deviation 6). The body's own
    /// editor never gets one but Esc, which blurs it: the engine is the
    /// body's only editor, and an edit or a selection made outside it would
    /// wipe its history (final review I-1). Tab and shift+Tab, which that
    /// editor indents with, do nothing; any other key runs what the keymap
    /// binds it to, as it would once the editor passed on it.
    fn route_forwarded(&mut self, id: FieldId, ev: KeyEvent) -> Ran {
        if id.is_modal() || id.insert_only() {
            let _quiet = crate::components::line_input::quiet();
            return Ran::routed(self.handle_key_routed(ev));
        }
        if id != FieldId::Body || (ev.code == KeyCode::Esc && ev.modifiers.is_empty()) {
            return Ran::routed(self.handle_key_routed(ev));
        }
        let indent =
            (ev.code == KeyCode::Tab && ev.modifiers.is_empty()) || ev.code == KeyCode::BackTab;
        match self.lookup_key(&KeyCombo::from_event(&ev)) {
            Some(a) if !indent => Ran::routed(self.update(a)),
            _ => Ran::default(),
        }
    }

    /// Whether a declined key goes on to today's routing (plan M3a
    /// Deviation 6): never a plain char, which would type in Normal; any
    /// other key in a dialog or a query box (plan M3b Deviation 6); a key
    /// the keymap binds; and the container's own named keys.
    fn forwards(&self, id: FieldId, ev: &KeyEvent) -> bool {
        let chord = ev.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if matches!(ev.code, KeyCode::Char(_)) && !chord {
            return false;
        }
        // A dialog's or a query box's own keys (its moves, its buttons, its
        // chords) all go on; its text input stays quiet (plan M3b
        // Deviation 6).
        if id.is_modal() || id.insert_only() {
            return true;
        }
        if self.lookup_key(&KeyCombo::from_event(ev)).is_some() {
            return true;
        }
        let plain = ev.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        match id.engine_kind() {
            EngineKind::Body => ev.code == KeyCode::Esc && ev.modifiers.is_empty(),
            EngineKind::Line => {
                plain
                    && matches!(
                        ev.code,
                        KeyCode::Esc
                            | KeyCode::Enter
                            | KeyCode::Tab
                            | KeyCode::BackTab
                            | KeyCode::Up
                            | KeyCode::Down
                            | KeyCode::PageUp
                            | KeyCode::PageDown
                    )
            }
        }
    }
}

/// Whether a mouse event syncs the session, between
/// [`App::vim_mouse_start`] and [`App::vim_mouse_end`]: a button's does,
/// and `left_press` tells a left press.
pub(super) struct MouseEdge {
    button: bool,
    left_press: bool,
}

/// The rest of the App calls these, and only these, to reach vim: each is
/// a whole operation, or answers `None` (or `false`) when vim has no part
/// in it, and the caller goes on with the plain path. Without the vim
/// profile each would be a no-op that answers the same.
impl App {
    /// The start of an input event, and anything that moved the caret or
    /// replaced a field's text outside one (an async result, a reload, the
    /// `$EDITOR` round-trip): the session catches up, so the next draw (the
    /// body's search prompt), key and paste see it as it is.
    pub fn vim_catch_up(&mut self) {
        self.sync_session(None, Cause::Before);
    }

    /// After a key: the session follows what the key did (spec §3.1).
    pub(super) fn vim_after_key(&mut self, ran: Ran) {
        self.sync_session(ran.open_as, Cause::Key { routed: ran.routed });
    }

    /// Before a mouse event: a button can open, move or close a field;
    /// motion and the wheel cannot, so only a button event syncs the
    /// session (spec §3.1). Of the drags only the left button's sweeps a
    /// selection: some terminals (Ghostty) report hover as `Drag(Right)`,
    /// and each sync reads the whole body (final review T4).
    pub(super) fn vim_mouse_start(
        &mut self,
        m: &ratatui::crossterm::event::MouseEvent,
    ) -> MouseEdge {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let button = matches!(
            m.kind,
            MouseEventKind::Down(_)
                | MouseEventKind::Up(_)
                | MouseEventKind::Drag(MouseButton::Left)
        );
        if button {
            self.sync_session(None, Cause::Before);
            self.vim.text_mouse = None;
            self.vim.separator_press = false;
        }
        MouseEdge { button, left_press: m.kind == MouseEventKind::Down(MouseButton::Left) }
    }

    /// After a mouse event: a button's settles the session. What the
    /// mouse did on the text of the field that has the caret now settles
    /// at its char (spec 2026-10-14); any other left press is a plain
    /// click, except one on a window separator, and a release does
    /// nothing, as in Vim.
    pub(super) fn vim_mouse_end(&mut self, edge: MouseEdge) {
        if !edge.button {
            return;
        }
        let separator = std::mem::take(&mut self.vim.separator_press);
        let how = match self.vim.text_mouse.take() {
            Some((field, at, mouse)) if self.field_caret() == FieldCaret::Live(field) => {
                Some(Settled::Mouse(at, mouse))
            }
            _ => (edge.left_press && !separator).then_some(Settled::Click),
        };
        self.sync_session(None, Cause::Mouse(how));
    }

    /// A left press on a window separator (`App::is_separator_press`, spec
    /// S7): the sidebar gutter, or in vim the response's header strip.
    /// Vim keeps Visual mode and a pending operator through a press there,
    /// so the end of the event settles nothing.
    pub(super) fn vim_separator_press(&mut self) {
        self.vim.separator_press = true;
    }

    /// A left press on `field`'s text at char `(row, col)`, with Vim's click
    /// count. In vim the engine makes the selection, so the field's own
    /// mouse does nothing: `true`. Outside vim and in a query box, which
    /// keeps its own selection, `false`.
    pub(super) fn vim_text_press(
        &mut self,
        field: FieldId,
        at: (usize, usize),
        clicks: u8,
    ) -> bool {
        self.vim_text_mouse(field, at, Mouse::Press(clicks))
    }

    /// A sweep on `field`'s text to char `(row, col)`: as
    /// [`Self::vim_text_press`].
    pub(super) fn vim_text_drag(&mut self, field: FieldId, at: (usize, usize)) -> bool {
        self.vim_text_mouse(field, at, Mouse::Drag)
    }

    /// A right press on the text of `field`, the field being edited, at
    /// char `(row, col)`: in vim it extends the selection (Vim's
    /// `'mousemodel'` `extend`). `false` outside vim and in a query box,
    /// where the menu stands.
    pub(super) fn vim_text_extend(&mut self, field: FieldId, at: (usize, usize)) -> bool {
        self.vim_text_mouse(field, at, Mouse::Extend)
    }

    /// Only on the field with the caret: the arm has opened it, and the
    /// end of the event settles there. Any other field's press stays the
    /// field's own (the file picker's path box is a query box's).
    fn vim_text_mouse(&mut self, field: FieldId, (row, col): (usize, usize), how: Mouse) -> bool {
        if self.profile() != KeyProfile::Vim
            || field.insert_only()
            || self.field_caret() != FieldCaret::Live(field)
        {
            return false;
        }
        self.vim.text_mouse = Some((field, Pos::new(row, col), how));
        true
    }

    /// The end of a key or mouse event: a V selection the keyboard left
    /// ends, and the response shows the app's last pattern (plan M5b
    /// Deviation 20): one a field searched, or the one a request switch put
    /// on screen a view too old for. Whether it ended a selection.
    pub(super) fn vim_event_end(&mut self) -> bool {
        let ended = self.end_visual_off_response();
        self.session.response.follow_last_pattern(&self.vim.search_memory);
        ended
    }

    /// Vim leaves Visual whenever the window changes (plan M5a Task 6): in
    /// the vim profile, a V selection on the response ends once the
    /// keyboard is anywhere else, by a window key, a click, a palette
    /// command that moves focus, another screen, or the jq bar taking the
    /// caret. Run at the boundary of each input event, so every way of
    /// leaving counts. A dialog or the palette over the response leaves it
    /// up: their commands act on the selection (copy, extract). The `:`
    /// line vim opened from V leaves it up only while a dialog is: Vim's
    /// Visual has ended once its `:'<,'>` line closes. A drag's character
    /// selection is not Visual, and an arrows shift selection is a GUI one:
    /// both stay, as on main. Whether it ended one.
    fn end_visual_off_response(&mut self) -> bool {
        let ex_closed = self.vim.ex_over_visual && self.modals.is_empty();
        if ex_closed {
            self.vim.ex_over_visual = false;
        }
        let away = ex_closed
            || self.screen != Screen::Main
            || self.focus != PaneId::Response
            || self.session.response.jq_focused();
        if !(away && self.profile() == KeyProfile::Vim && self.session.response.line_selection()) {
            return false;
        }
        self.session.response.lines(crate::action::LinesOp::End);
        true
    }

    /// The response with the app's last search pattern lent to it: its
    /// keys and its search steps read and set the one pattern every engine
    /// shares (plan M5b Deviation 20).
    pub(crate) fn with_response_search<R>(
        &mut self,
        f: impl FnOnce(
            &mut crate::components::response::Response,
            &mut crate::vim::engine::SearchMemory,
        ) -> R,
    ) -> R {
        f(&mut self.session.response, &mut self.vim.search_memory)
    }

    /// Whether `id`'s keystrokes are in its engine session, and if so
    /// whether it holds edits (plan M3a Deviation 7). `None` when the
    /// screen engine does not own `id`: the field's own `edited()` counts.
    pub(crate) fn vim_edited(&self, id: FieldId) -> Option<bool> {
        (self.vim.glue.owned() == Some(id)).then(|| self.vim.glue.edited(id.engine_kind()))
    }

    /// A key's `Step::Sync`: the session follows the caret now, opened as
    /// the key asked, so its engine keys reach the field it just opened.
    pub(crate) fn vim_sync_now(&mut self, open_as: Option<OpenAs>) {
        self.sync_session(open_as, Cause::Before);
    }

    /// The app history took the session's edits as a step: what the fields
    /// hold now is where their next step starts (plan M3a Deviation 7).
    pub(crate) fn vim_flush(&mut self) {
        self.vim.glue.flush();
    }

    /// Whether the live vim field owns the undo keys: while it has live
    /// edits or a step either way (amendment C1: before the first committed
    /// step the live edit still counts, or ctrl+z in Insert would undo an
    /// older app step and lose the typed text). `None` when no vim field
    /// has the caret.
    pub(crate) fn vim_field_has_history(&self) -> Option<bool> {
        let id = self.live_engine_field()?;
        let kind = id.engine_kind();
        let g = self.glue_of(id);
        Some(g.edited(kind) || g.can_undo(kind) || g.can_redo(kind))
    }

    /// Whether the engine owns the body's keys (its motions, its chords).
    pub(crate) fn vim_owns_body(&self) -> bool {
        self.vim.glue.owned() == Some(FieldId::Body)
    }

    /// The open request's slug changed in place from `from` to the one the
    /// editor holds now (a rename, a space rename, their undo or redo, a
    /// scratch saved under a name): the body's engine state follows it
    /// (amendment C2).
    pub(crate) fn vim_body_moved(&mut self, from: Option<&str>) {
        self.vim.glue.follow_body(from, self.editor.slug.as_deref());
    }

    /// The editor's body is a new buffer with no slug to tell it apart (a
    /// fresh scratch): the body's engine state goes with the old one.
    pub(crate) fn vim_drop_body(&mut self) {
        self.vim.glue.drop_body(&mut self.vim.search_memory);
    }

    /// The live engine field's mode and pending state, for the router's
    /// field lookahead (spec §3.7).
    pub(crate) fn vim_engine_view(&self) -> Option<crate::keyroute::EngineView> {
        self.live_engine_field().map(|id| self.glue_of(id).view())
    }

    /// How the footer spells keys for the live engine field: its idle
    /// Normal's field class, else typing. `None` when no vim field has the
    /// caret.
    pub(crate) fn vim_field_spell(
        &self,
        surface: crate::keyroute::Surface,
    ) -> Option<crate::keyroute::FieldSpell> {
        use crate::keyroute::FieldSpell;
        let id = self.live_engine_field()?;
        let glue = self.glue_of(id);
        Some(if glue.mode() == Mode::Normal && !glue.view().pending {
            FieldSpell::Normal(table::field_class(id, surface))
        } else {
            FieldSpell::Typing
        })
    }

    /// The live engine field's half-typed keys for the footer (spec §3.8),
    /// empty when there are none.
    pub(crate) fn vim_field_echo(&self) -> String {
        self.live_engine_field().map(|id| self.glue_of(id).echo()).unwrap_or_default()
    }

    /// The vim mode word (spec §3.8): what is taking the keys. A field with
    /// a live session shows its engine's mode, on either glue (plan M3b
    /// Deviation 1); a screen field not synced yet shows the screen
    /// engine's; any other field with the caret types (`INSERT`: a dialog
    /// field before its session opens, which opens it in Insert, and the
    /// query boxes); the response's line selection is `V-LINE` (plan M5a
    /// Deviation 18); everything else is `NORMAL`. `None` in the arrows
    /// profile.
    pub fn mode_indicator(&self) -> Option<&'static str> {
        if self.profile() != KeyProfile::Vim {
            return None;
        }
        Some(match (self.live_engine_field(), self.field_caret()) {
            (Some(id), _) => mode_word(self.glue_of(id).mode()),
            // A screen field whose session has not synced yet (opened with
            // no event) shows the screen engine's mode, as before M3b: the
            // sync opens it in Normal.
            (None, FieldCaret::Live(id)) if !id.is_modal() && !id.insert_only() => {
                mode_word(self.vim.glue.mode())
            }
            (None, FieldCaret::Live(_)) => "INSERT",
            (None, _) if matches!(self.surface(), crate::keyroute::Surface::ResponseLines(_)) => {
                "V-LINE"
            }
            (None, _) => "NORMAL",
        })
    }

    /// The mode and the echo as the footer paints them: `NORMAL  3`.
    pub fn vim_status(&self) -> Option<String> {
        let mode = self.mode_indicator()?;
        let echo = self.key_echo();
        Some(if echo.is_empty() { mode.to_string() } else { format!("{mode}  {echo}") })
    }

    /// The next field an add verb opens opens in Insert (spec §5.6).
    pub(crate) fn vim_open_next_in_insert(&mut self) {
        self.vim.add_opens_insert = true;
    }

    /// The `:` line's history as `ui.toml` held it at startup.
    pub(crate) fn vim_restore_history(&mut self, history: crate::cmdline::CmdlineHistory) {
        self.vim.cmdline_history = history;
    }

    /// The app is quitting: the `:` line's history is written with the
    /// palette's usage. The app is exiting and ui.toml is app state, so a
    /// failed write has no one left to tell.
    pub(crate) fn vim_on_quit(&mut self) {
        let _ = self.config.save_cmdline_history(&self.vim.cmdline_history);
    }

    /// The key router ran a key's steps from the history's change count
    /// `before` (spec §5.12): the list change it ran is `.`'s, once the
    /// event's capture shows it changed something.
    pub(crate) fn vim_note_change(&mut self, before: u64) {
        if let Some(change) = self.router.take_change() {
            self.vim.dot_pending = DotPending::Made { change, before };
        }
    }

    /// A modal closed (`popped`) with its result about to run, from the
    /// history's change count `before`. Vim's command-line commands leave
    /// `.` alone (probes D9, D16), unless a field open under the line
    /// closes with it, or a field session (the body's, which never closes)
    /// holds edits the line's command may flush: that edit may be the last
    /// change (plan M5b Deviation 6).
    pub(crate) fn vim_palette_ran(&mut self, popped: Option<&Modal>, before: u64) {
        if matches!(popped, Some(Modal::Palette(_)))
            && !self.screen_field_open()
            && !self.field_gate()
        {
            self.vim.dot_pending = DotPending::Kept { before };
        }
    }

    /// A modal closed, by its own result or by a click off it. Vim keeps
    /// each line its `:` closed with, Enter, Esc or abandoned (probes H1,
    /// H6; plan M5b Deviations 13 and 17).
    pub(crate) fn vim_line_closed(&mut self, popped: Option<&Modal>) {
        if let Some(Modal::Palette(p)) = popped
            && p.is_ex()
        {
            self.vim.cmdline_history.add(p.input());
        }
    }

    /// Settles what the last event did to `.` (plan M5b Deviations 5–8),
    /// now that its undo capture has run: a list change that changed
    /// something becomes `.`; a change the palette ran leaves `.` as it
    /// was; any other change since `.` was made makes it stale. The event
    /// loop calls it right after each `capture_undo` (amendment A4.4), so a
    /// change a later non-input event records is never credited to a key.
    pub fn vim_settle_dot(&mut self) {
        let now = self.history.changes();
        let vim = &mut self.vim;
        match std::mem::take(&mut vim.dot_pending) {
            DotPending::Made { change, before } if now != before => {
                vim.dot = Some(Dot { change, at: now })
            }
            DotPending::Kept { before } => {
                if let Some(dot) = vim.dot.as_mut()
                    && dot.at == before
                {
                    dot.at = now;
                }
            }
            _ => {}
        }
        if vim.dot.is_some_and(|d| d.at != now) {
            vim.dot = None;
        }
    }

    /// The actions only vim's keys make (`App::apply` hands them here).
    pub(super) fn apply_vim(&mut self, action: Action) -> bool {
        match action {
            Action::WriteQuit | Action::ForceWrite { quit: true } => {
                if !self.leaving_unsaved() {
                    return self.apply(Action::ForceQuit);
                }
                if self.editor.slug.is_none() {
                    return self.apply(Action::PromptSaveScratch(Box::new(Action::ForceQuit)));
                }
                // A failed write keeps the app open.
                let force = matches!(action, Action::ForceWrite { .. });
                self.save_request_checked(Some(Action::ForceQuit), force)
            }
            Action::ForceWrite { quit: false } => self.save_request_checked(None, true),
            Action::CloseWindow { write, force } => {
                // Manage by the keyboard's surface, as the line's caption and
                // the quit chip read it. (Over a dialog, Manage's or the main
                // screen's, `apply` refuses every window verb first: no quit
                // while a dialog is open, task #85.)
                if !table::Surfaces::MANAGE.has(self.surface()) {
                    return self.apply(match (write, force) {
                        (false, false) => Action::GatedQuit,
                        (false, true) => Action::ForceQuit,
                        (true, false) => Action::WriteQuit,
                        (true, true) => Action::ForceWrite { quit: true },
                    });
                }
                if force {
                    // `ZQ` and `:q!` abandon a field open on the screen, as
                    // Vim's `!` abandons changes, so a field whose close
                    // would refuse cannot keep the screen. Dropped here,
                    // since `CloseScreen` would save them.
                    self.varmanager.end_edits();
                    self.settings.end_edit();
                } else {
                    // A field open on the screen closes first, as the line's
                    // `:q` and `ZZ` close it; one that refuses keeps the
                    // screen (the final review's ruling I-2).
                    self.last_action_failed = false;
                    self.update(Action::CloseField);
                    if self.last_action_failed {
                        return true;
                    }
                }
                self.update(Action::CloseScreen)
            }
            Action::OpenExPalette => {
                use crate::components::palette::PaletteState;
                let scope = self.ex_scope();
                self.vim.ex_over_visual |= self.profile() == KeyProfile::Vim
                    && self.focus == PaneId::Response
                    && self.session.response.line_selection();
                let mut palette = PaletteState::new_ex(
                    &self.usage,
                    crate::usage::now(),
                    scope,
                    self.vim.cmdline_history.lines(),
                )
                .opened_at(self.spell_ctx(), self.key_context());
                if let Some((name, offers)) = self.tip_offers() {
                    palette = palette.with_tip(&name, &offers);
                }
                self.push_modal(Modal::Palette(palette));
                true
            }
            Action::NoHlSearch => {
                // Vim's `:noh` turns the highlight off in every window: the
                // one memory every engine borrows (plan M5b Deviation 20).
                self.session.response.no_hlsearch(&mut self.vim.search_memory);
                true
            }
            Action::ResponseFind(crate::action::ResponseFind::Next { reverse, count }) => {
                self.session.response.find_next(reverse, count, &mut self.vim.search_memory)
            }
            Action::ResponseFind(crate::action::ResponseFind::Word { backward, count }) => {
                self.session.response.find_word(backward, count, &mut self.vim.search_memory)
            }
            Action::TreeFold { op, count } => self.session.response.fold(op, count),
            Action::ResponseMove(m) => self.session.response.motion(m),
            Action::ResponseExtend(m) => self.session.response.extend(m),
            Action::ResponseLines(op) => {
                let had = self.session.response.line_selection();
                match self.session.response.lines(op) {
                    Some(lines) => {
                        let n = lines.len();
                        self.vim.glue.yank_lines(&lines);
                        // Vim's `'report'` (2): three lines or more say so.
                        if n >= 3 {
                            self.update(Action::ShowToast(
                                format!("{n} lines yanked"),
                                ToastKind::Info,
                            ));
                        }
                        true
                    }
                    None => {
                        had != self.session.response.line_selection()
                            || op == crate::action::LinesOp::Swap
                    }
                }
            }
            Action::RepeatChange { count } => {
                let Some(dot) = self.vim.dot else { return false };
                let change =
                    crate::keyroute::Change { count: count.or(dot.change.count), ..dot.change };
                let surface = self.surface();
                let Some(steps) =
                    crate::keyroute::replay(change, surface, self.repeat_cap(surface))
                else {
                    return false;
                };
                let before = self.history.changes();
                let ran = self.run_steps(steps);
                // The repeat is the change now, its count the new one
                // (probes D3, D11).
                self.vim.dot_pending = DotPending::Made { change, before };
                ran.changed
            }
            Action::SidebarFold { op, count } => {
                use crate::components::sidebar::FoldChange;
                let real = self.sidebar.fold_cursor();
                match self.sidebar.fold_change(
                    op,
                    count,
                    real.as_ref(),
                    &self.expanded(),
                    &self.active_space(),
                ) {
                    FoldChange::NoFold => {
                        self.update(Action::ShowToast(table::NO_FOLD.into(), ToastKind::Info))
                    }
                    FoldChange::Same => false,
                    FoldChange::Expanded(set) => {
                        if let Some(p) = self.project_mut() {
                            // The setter persists, as `h` and `l` do: a fold
                            // is no undo step (plan M5b Deviation 12).
                            p.set_expanded(set);
                        }
                        self.refresh_sidebar();
                        if let Some(real) = real {
                            self.sidebar.show_fold_cursor(real);
                        }
                        true
                    }
                }
            }
            other => {
                debug_assert!(false, "{other:?} is not a vim action");
                false
            }
        }
    }
}
