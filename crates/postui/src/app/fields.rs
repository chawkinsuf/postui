//! The vim profile's text fields (piece 4 spec §3.3, §3.7; plan M3a): the
//! session edges, keys into the engine, and what the engine hands back.
//! Every method here is `impl App`. The engine itself is reached only
//! through `keyroute::engine_glue`.

use super::*;
use crate::config::KeyProfile;
use crate::keyroute::engine_glue::{
    self, AppRequest, Edge, FieldBuf, FieldSession, Glue, Mode, Note, OpenAs, OpenSeat, Outcome, Pos, Settled, Shape,
    Start, ViewCtx,
};
use crate::keyroute::notation::Tok;
use crate::keyroute::table::{self, Intent, Match};
use crate::keyroute::{EngineKind, FieldCaret, FieldId, Step};

/// What routing one key did (spec §3.1): whether to redraw, the open mode
/// a table row asked for (plan M3a Deviation 3), and whether a step other
/// than the engine ran, which makes the end-of-event sync settle the
/// session (spec §3.3: "a key the router ran itself").
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ran {
    pub changed: bool,
    pub open_as: Option<OpenAs>,
    pub routed: bool,
}

impl Ran {
    /// Today's routing ran.
    pub(crate) fn routed(changed: bool) -> Self {
        Self { changed, routed: true, open_as: None }
    }

    /// The router or the engine took the key.
    pub(crate) fn consumed() -> Self {
        Self { changed: true, ..Self::default() }
    }

    pub(crate) fn and(self, o: Ran) -> Self {
        Self { changed: self.changed || o.changed, open_as: o.open_as.or(self.open_as), routed: self.routed || o.routed }
    }
}

/// What an event was, for the session's settle (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    /// The start of an event: catch up with what changed since the last.
    Before,
    Key { routed: bool },
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

/// `id`'s one-line buffer, or none when that field is not open (or is the
/// body). Takes the parts of `App` that hold one, which leave the body
/// free, so a carry out of the body can hold both (amendment C3).
fn line_buf<'a>(
    id: FieldId,
    url: &'a mut LineInput,
    table: &'a mut crate::components::table_editor::TableEditorState,
    varmanager: &'a mut VarManager,
    settings: &'a mut crate::components::settings::SettingsTab,
) -> Option<FieldBuf<'a>> {
    match id {
        FieldId::Url => Some(FieldBuf::Line(url)),
        FieldId::TableCell { .. } => table.editing.as_mut().map(|e| FieldBuf::Line(&mut e.input)),
        FieldId::VmForm(_) => varmanager.form.editing.as_mut().map(|e| FieldBuf::Line(&mut e.input)),
        FieldId::VmGrid { .. } => varmanager.grid.editing.as_mut().map(|e| FieldBuf::Line(&mut e.input)),
        FieldId::Settings(_) => {
            if settings.editing.is_some() {
                Some(FieldBuf::Line(settings.input_mut()))
            } else {
                None
            }
        }
        _ => None,
    }
}

impl App {
    /// The glue `id` runs on and `id`'s buffer at once (disjoint fields),
    /// or no buffer when that field is not open. A dialog's field lends the
    /// dialog engine and the top dialog's input (plan M3b Deviation 1).
    pub(crate) fn session_parts(&mut self, id: FieldId) -> (&mut Glue, Option<FieldBuf<'_>>) {
        match id {
            FieldId::Body => {
                (&mut self.glue, Some(FieldBuf::Body(&mut self.editor.body, &mut self.editor.body_visual)))
            }
            FieldId::ModalInput(i) => (&mut self.modal_glue, self.modals.text_input_mut(i).map(FieldBuf::Line)),
            _ => (
                &mut self.glue,
                line_buf(id, &mut self.editor.url, &mut self.editor.table, &mut self.varmanager, &mut self.settings),
            ),
        }
    }

    /// The glue, the body and `to`'s buffer at once, for a carry out of the
    /// body (amendment C3).
    fn body_and(&mut self, to: FieldId) -> (&mut Glue, FieldBuf<'_>, Option<FieldBuf<'_>>) {
        let body = FieldBuf::Body(&mut self.editor.body, &mut self.editor.body_visual);
        let to = line_buf(to, &mut self.editor.url, &mut self.editor.table, &mut self.varmanager, &mut self.settings);
        (&mut self.glue, body, to)
    }

    /// The field with an engine session now, and whether it has the caret
    /// (spec §3.3). A modal over an engine field parks it, whether the modal
    /// has a field of its own or not. Only in the vim profile.
    pub(crate) fn engine_field(&self) -> Option<(FieldId, bool)> {
        if self.profile() != KeyProfile::Vim {
            return None;
        }
        match self.field_caret() {
            FieldCaret::Live(id) if !id.is_modal() && !id.insert_only() => Some((id, true)),
            FieldCaret::Live(_) => self.screen_field().filter(|id| !id.insert_only()).map(|id| (id, false)),
            FieldCaret::Parked(id) if !id.insert_only() => Some((id, false)),
            FieldCaret::Parked(_) | FieldCaret::None => None,
        }
    }

    /// Brings the engine session in line with where the caret is (spec
    /// §3.3): open, carry, close, keep or rebase, then settle after a click
    /// or a key the router ran itself. Runs at the start and the end of
    /// every key and mouse button event (plan M3a Deviation 2).
    pub(crate) fn sync_session(&mut self, open_as: Option<OpenAs>, cause: Cause) {
        let profile = self.profile();
        let was = self.glue.profile();
        let switched_in = profile == KeyProfile::Vim && was == Some(KeyProfile::Arrows);
        self.glue.set_profile(profile);
        if profile != KeyProfile::Vim {
            // Arrows has no Normal mode: an open field goes on as main's
            // modeless field, its text and caret untouched (spec §4.3).
            // Leaving Insert would step the caret back off the end, so it
            // is put back where it was.
            if let Some(id) = self.glue.owned() {
                let caret = self.session_parts(id).1.map(|buf| buf.caret());
                self.close_session(id);
                if let Some(at) = caret {
                    self.put_caret(id, at);
                }
                self.unpaint(id, true);
                self.glue.set_session(FieldSession::None);
            }
            // The dialog session goes the same way, and its engine gives
            // the registers back (plan M3b Deviation 1).
            if let Some((_, id)) = self.modal_session.take() {
                let caret = self.session_parts(id).1.map(|buf| buf.caret());
                self.close_session(id);
                if let Some(at) = caret {
                    self.put_caret(id, at);
                }
                self.unpaint(id, true);
                self.glue.swap_registers(&mut self.modal_glue);
                self.modal_glue.set_session(FieldSession::None);
            }
            // A body session that closed before the switch left edtui in
            // Normal, where the arrows body binds no keys: give it its
            // Insert back whether or not a session is still owned.
            if was == Some(KeyProfile::Vim) {
                self.unpaint(FieldId::Body, true);
            }
            return;
        }
        self.glue.own_body(self.editor.slug.as_deref());
        let now = self.engine_field();
        let mouse = matches!(cause, Cause::Mouse(_));
        let edge = engine_glue::edge(self.glue.owned(), now.map(|(id, _)| id));
        match edge {
            Edge::Idle => {}
            Edge::Open(id) => {
                // Arrows to vim with a field open: it was being typed in, so
                // it goes on in Insert (spec §4.3).
                let open = if switched_in { Some(OpenAs::Insert(OpenSeat::Keep)) } else { open_as };
                self.open_session(id, open, !mouse, Start::Normal);
            }
            Edge::Carry(from, to) => self.carry_session(from, to),
            // Spec §3.3: the close edge also drops the router's pending
            // keys (amendment A4-1).
            Edge::Close(id) => {
                self.close_session(id);
                self.router.clear();
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
        self.glue.set_session(match now {
            Some((id, true)) => FieldSession::Live(id),
            Some((id, false)) => FieldSession::Parked(id),
            None => FieldSession::None,
        });
        let how = match cause {
            Cause::Mouse(how) => how,
            Cause::Key { routed: true } if matches!(edge, Edge::Same(_)) => Some(Settled::Key),
            _ => None,
        };
        if let (Some(how), Some(id)) = (how, self.glue.live()) {
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
    /// a dialog session opens and come back when it closes.
    fn sync_modal_session(&mut self, open_as: Option<OpenAs>, cause: Cause, switched_in: bool) {
        let now = self.modal_engine_field();
        let was = self.modal_session;
        let top = self.modals.top_serial();
        let fresh = top != self.modal_seen;
        self.modal_seen = top;
        let mouse = matches!(cause, Cause::Mouse(_));
        // A field opens in Insert when its dialog is new, on a click, after
        // a dialog over it closed, and when vim was just switched on; from
        // the keyboard in a dialog that stays, in Normal (Deviation 3).
        let default = if fresh || mouse || switched_in { Start::Insert } else { Start::Normal };
        match (was, now) {
            (None, None) => {}
            (Some(a), Some(b)) if a == b => {
                let (glue, buf) = self.session_parts(b.1);
                if let Some(mut buf) = buf
                    && glue.differs(&buf)
                {
                    glue.rebase(&mut buf);
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
                self.modal_glue.leave_ghost();
                self.open_session(to, open_as, !mouse, default);
            }
            (None, Some((_, to))) => {
                self.glue.swap_registers(&mut self.modal_glue);
                self.open_session(to, open_as, !mouse, default);
            }
            // The field closed in its own dialog (`j` to the buttons), or
            // its dialog was popped or covered: then it is left through the
            // ghost, as above, since its index would name the top's field.
            (Some((from_dialog, from)), None) => {
                if Some(from_dialog) == top {
                    self.close_session(from);
                } else {
                    self.modal_glue.leave_ghost();
                }
                self.glue.swap_registers(&mut self.modal_glue);
                self.router.clear();
            }
        }
        self.modal_session = now;
        self.modal_glue.set_session(now.map_or(FieldSession::None, |(_, id)| FieldSession::Live(id)));
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
            FieldCaret::Live(id @ FieldId::ModalInput(_)) => self.modals.top_serial().map(|s| (s, id)),
            _ => None,
        }
    }

    /// `id` got the caret: the engine enters it in the mode the key asked
    /// for, else `default`: Normal on the screen; a dialog field's rule is
    /// plan M3b Deviation 3; a query box is always Insert-only (spec §5.6;
    /// plan M3a Deviation 12). A keyboard open drops a GUI selection (the
    /// Settings field selects all for arrows). A press that opened a
    /// one-line field keeps the anchor it planted, which seating the caret
    /// drops, so a drag that follows selects (final review I-3; the body's
    /// anchor lives apart from its caret).
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
                state.mode = if modeless { edtui::EditorMode::Insert } else { edtui::EditorMode::Normal };
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
        [self.modal_glue.live(), self.glue.live()]
            .into_iter()
            .flatten()
            .find(|&id| caret == FieldCaret::Live(id))
    }

    /// The engine `id` runs on: the dialog engine for a dialog's field
    /// (plan M3b Deviation 1), else the screen's.
    pub(crate) fn glue_of(&self, id: FieldId) -> &Glue {
        if id.is_modal() { &self.modal_glue } else { &self.glue }
    }

    /// Catches the session up with what moved the caret or replaced a
    /// field's text outside a key or a mouse button: an async result, a
    /// reload, the `$EDITOR` round-trip. The main loop runs it after each,
    /// so the next draw (the body's search prompt) and the next paste see
    /// the session as it is.
    pub fn catch_up_field_session(&mut self) {
        self.sync_session(None, Cause::Before);
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
        let view = self.view_ctx(id);
        // Read before the key: whether a `u` or ctrl+r it hands back means
        // the field had nothing to step (final review M-1).
        let idle = self.glue_of(id).undo_ready();
        let out = {
            let (glue, buf) = self.session_parts(id);
            glue.handle(ev, &mut buf?, &view)
        };
        Some(match out {
            Outcome::Consumed { note, request, .. } => {
                self.engine_feedback(note, request);
                self.after_typed(id, ev);
                Ran::consumed()
            }
            Outcome::Declined { count, keys } => self.route_declined(id, count, keys, idle),
        })
    }

    /// The app's undo path (`step_history`) reaching a vim field: the
    /// field's own history first (spec §3.7). `None` when no engine field
    /// has the caret. `Some(false)` when it has nothing to step that way,
    /// or the engine is not in idle Normal, where the undo chords do
    /// nothing (plan M3a Deviation 8).
    pub(crate) fn engine_field_undo(&mut self, redo: bool) -> Option<bool> {
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
        let view = self.view_ctx(id);
        let out = {
            let (glue, buf) = self.session_parts(id);
            glue.handle(key, &mut buf?, &view)
        };
        if let Outcome::Consumed { note, request, .. } = out {
            self.engine_feedback(note, request);
        }
        Some(true)
    }

    /// The body's text replaced as one engine step while the body has an
    /// engine session, live or parked (spec §3.7: format, minify, clear).
    /// `false` leaves the rewrite to the caller.
    pub(crate) fn engine_replace_body(&mut self, text: &str) -> bool {
        if self.glue.owned() != Some(FieldId::Body) {
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
    /// pastes.
    pub(crate) fn engine_paste(&mut self, text: &str) -> Option<bool> {
        let id = self.live_engine_field()?;
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
        Some(self.update(Action::Render))
    }

    /// A picked `{{token}}` into the field the picker was opened from
    /// (spec §3.7): at the caret in Insert, over the selection in Visual
    /// (as a paste), else by the bracketed-paste rule (plan M3a Deviation
    /// 13). Insert ctrl+o counts as Insert: the edit resumes it first, as
    /// `Glue::rebase` does. Only the fields main inserts into: the URL
    /// line, a cell, the body. `false` leaves it to today's path.
    pub(crate) fn engine_insert(&mut self, text: &str) -> bool {
        let Some(id) = self.glue.owned() else { return false };
        if !matches!(id, FieldId::Url | FieldId::TableCell { .. } | FieldId::Body) {
            return false;
        }
        let typing = matches!(self.glue.mode(), Mode::Insert | Mode::Replace | Mode::InsertNormal { .. });
        let (glue, buf) = self.session_parts(id);
        let Some(mut buf) = buf else { return false };
        let edit = if typing {
            engine_glue::Edit::AtCaret(text)
        } else if let Some((from, to)) = glue.visual_span(&mut buf) {
            engine_glue::Edit::Replace(from, to, text)
        } else {
            engine_glue::Edit::Append(text)
        };
        glue.edit(&mut buf, &edit);
        true
    }

    /// The live engine field's Visual text, which ctrl+c and cmd+c copy
    /// (the user's decision, final review I-2): a mouse selection in a vim
    /// field becomes Visual, so without it ctrl+c would quit.
    pub(crate) fn engine_visual_text(&mut self) -> Option<String> {
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
    pub(crate) fn mirror_body_search(&mut self) {
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
        let found = self.glue.body_search(&mut self.editor.body, &mut self.editor.body_visual, rows);
        self.editor.body_search = found;
    }

    /// `{{` typed into the URL line or a cell opens the variable picker, as
    /// it does on main (`Editor::handle_key`).
    fn after_typed(&mut self, id: FieldId, ev: KeyEvent) {
        let typing = matches!(self.glue_of(id).mode(), Mode::Insert | Mode::Replace);
        if ev.code != KeyCode::Char('{') || !typing || !matches!(id, FieldId::Url | FieldId::TableCell { .. }) {
            return;
        }
        let braces =
            matches!(self.session_parts(id).1, Some(FieldBuf::Line(input)) if input.ends_with_at_cursor("{{"));
        if braces {
            self.update(Action::OpenVarPicker { completing: true });
        }
    }

    /// A key the engine handed back (plan M3a Deviation 6). `idle`: the
    /// engine was in idle Normal before the key (`Glue::undo_ready`).
    fn route_declined(&mut self, id: FieldId, count: Option<usize>, keys: Vec<KeyEvent>, idle: bool) -> Ran {
        let n = count.unwrap_or(1);
        // The field had nothing to step: the app history's turn (spec §3.7).
        // Only from idle Normal: in Visual, or with an operator pending, the
        // engine hands `u` or ctrl+r back as a key it has no use for, and
        // Vim does nothing (final review M-1).
        match keys.last().and_then(Tok::of) {
            Some(Tok::Char('u')) if idle => return self.run_steps(vec![Step::Action(Action::Undo); n]),
            Some(Tok::Ctrl('r')) if idle => return self.run_steps(vec![Step::Action(Action::Redo); n]),
            _ => {}
        }
        let cap = self.repeat_cap(self.surface());
        // A field row the engine handed back whole (M4's `gt`).
        let toks: Option<Vec<Tok>> = keys.iter().map(Tok::of).collect();
        if let Some(toks) = toks
            && let Match::Complete(b) = table::lookup_class(&toks, KeyProfile::Vim, table::field_class(id, self.surface()))
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
            let repeat = matches!(ev.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown);
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
        let indent = (ev.code == KeyCode::Tab && ev.modifiers.is_empty()) || ev.code == KeyCode::BackTab;
        match self.keymap.lookup(&KeyCombo::from_event(&ev)) {
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
        if self.keymap.lookup(&KeyCombo::from_event(ev)).is_some() {
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
