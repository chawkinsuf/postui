//! The engine boundary (piece 4 spec §3.7): the one place piece 4 calls
//! piece 3's text engine. `Glue` owns the engine, the state Vim keeps per
//! buffer (§3.11: a one-line field's lasts one open, the body's lasts while
//! its request stays open) and the field session (§3.3). `App` lends it the
//! buffer of the field with the caret for each call (`App::session_parts`).

use super::ctx::{EngineKind, EngineView, FieldId};
use crate::components::line_input::LineInput;
use crate::config::KeyProfile;
use crate::vim::engine::{BodyBuf, BodyVisual, BufState, Engine, OneLineBuf, Splicer, Target, TextBuf};
pub use crate::vim::engine::{AppRequest, Dir, Mode, Note, Outcome, Pos, SearchLine, Seat, Settled, Shape, Start, ViewCtx};
use edtui::{EditorState, Index2, Lines};
use ratatui::crossterm::event::KeyEvent;

/// The buffer of the field with the caret, lent for one engine call.
pub enum FieldBuf<'a> {
    Line(&'a mut LineInput),
    Body(&'a mut EditorState, &'a mut BodyVisual),
}

impl FieldBuf<'_> {
    pub fn kind(&self) -> EngineKind {
        match self {
            FieldBuf::Line(_) => EngineKind::Line,
            FieldBuf::Body(..) => EngineKind::Body,
        }
    }

    pub fn text(&self) -> String {
        match self {
            FieldBuf::Line(input) => input.text().to_string(),
            FieldBuf::Body(state, _) => state.lines.to_string(),
        }
    }

    pub fn caret(&self) -> Pos {
        match self {
            FieldBuf::Line(input) => Pos::new(0, input.cursor()),
            FieldBuf::Body(state, _) => Pos::new(state.cursor.row, state.cursor.col),
        }
    }
}

/// Which field has an engine session (spec §3.3). A parked field sits under
/// a modal that has its own field or none: the engine is not called, so its
/// mode and text wait untouched (plan M3a Deviation 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldSession {
    #[default]
    None,
    Live(FieldId),
    Parked(FieldId),
}

/// How a key asked a field to open (spec §3.1, §5.6). Applied by the
/// end-of-event sync when the key opened a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAs {
    Normal,
    Insert(OpenSeat),
}

/// Where an opening key puts the caret (plan M3a Deviation 12): piece 3's
/// `Seat`, plus `After` (one past the caret, the URL line's `a`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSeat {
    Keep,
    After,
    End,
    FirstNonBlank,
}

/// A change made outside the key path (spec §3.7: a picked `{{token}}`,
/// format, minify, paste outside Insert): one engine undo step, not
/// repeatable with `.`. One-line text must already be flattened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit<'a> {
    /// At the caret, as Insert would type it: the caret ends after it.
    AtCaret(&'a str),
    /// Vim's bracketed paste outside Insert (`nv_edit()` with `K_PS`):
    /// after the caret's char, or before it at column 0; the caret ends on
    /// the last char put (plan M3a Deviation 13).
    Append(&'a str),
    /// The whole text, the caret kept where it was (the engine clamps it).
    Whole(&'a str),
    /// `[from, to)` replaced, the caret on the last char put: a paste or
    /// a picked token over a Visual selection ([`Glue::visual_span`]), as
    /// Vim and GUI editors replace a selection (the user's ruling, final
    /// review).
    Replace(Pos, Pos, &'a str),
}

impl Edit<'_> {
    fn apply<B: TextBuf>(&self, s: &mut Splicer<'_, B>) {
        let caret = s.buf().cursor();
        match *self {
            Edit::AtCaret(text) => {
                s.splice(caret, caret, text);
                s.set_cursor(end_of(caret, text));
            }
            Edit::Append(text) => {
                let len = s.buf().line_len(caret.row);
                let at = if caret.col == 0 { caret } else { Pos::new(caret.row, (caret.col + 1).min(len)) };
                s.splice(at, at, text);
                let end = end_of(at, text);
                s.set_cursor(Pos::new(end.row, end.col.saturating_sub(1)));
            }
            Edit::Whole(text) => {
                let last = s.buf().line_count() - 1;
                let end = Pos::new(last, s.buf().line_len(last));
                s.splice(Pos::new(0, 0), end, text);
                s.set_cursor(caret);
            }
            Edit::Replace(from, to, text) => {
                s.splice(from, to, text);
                let end = end_of(from, text);
                s.set_cursor(Pos::new(end.row, end.col.saturating_sub(1)));
            }
        }
    }
}

/// The Visual selection with its fixed end at `anchor` and the caret as
/// the moving one, as a `[from, to)` range of `b` (see
/// [`Glue::visual_span`]).
fn span_of<B: TextBuf>(b: &B, anchor: Pos, shape: Shape) -> (Pos, Pos) {
    let last = b.line_count() - 1;
    let caret = b.cursor();
    let (lo, hi) = if anchor <= caret { (anchor, caret) } else { (caret, anchor) };
    let (lo_row, hi_row) = (lo.row.min(last), hi.row.min(last));
    let len = b.line_len(hi_row);
    if shape == Shape::Line {
        return (Pos::new(lo_row, 0), Pos::new(hi_row, len));
    }
    let to = if hi.col < len {
        Pos::new(hi_row, hi.col + 1)
    } else if hi_row < last {
        Pos::new(hi_row + 1, 0)
    } else {
        Pos::new(hi_row, len)
    };
    (Pos::new(lo_row, lo.col.min(b.line_len(lo_row))), to)
}

/// Where `text` put at `at` ends.
fn end_of(at: Pos, text: &str) -> Pos {
    match text.rsplit_once('\n') {
        Some((head, tail)) => Pos::new(at.row + head.matches('\n').count() + 1, tail.chars().count()),
        None => Pos::new(at.row, at.col + text.chars().count()),
    }
}

/// What the caret did between two syncs, by the field with an engine
/// session before and the one with it now (spec §3.3's edge table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Idle,
    Open(FieldId),
    Carry(FieldId, FieldId),
    Close(FieldId),
    Same(FieldId),
}

pub fn edge(was: Option<FieldId>, now: Option<FieldId>) -> Edge {
    match (was, now) {
        (None, None) => Edge::Idle,
        (None, Some(b)) => Edge::Open(b),
        (Some(a), None) => Edge::Close(a),
        (Some(a), Some(b)) if a == b => Edge::Same(a),
        (Some(a), Some(b)) => Edge::Carry(a, b),
    }
}

/// What the body draws for the engine's search (spec §5.6): the open
/// prompt, every match on the rows asked for (end exclusive), and the match
/// Enter would land on while the prompt is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BodySearch {
    pub prompt: Option<SearchLine>,
    pub matches: Vec<(Pos, Pos)>,
    pub current: Option<(Pos, Pos)>,
}

/// The session buffer's text and caret after the engine last touched it
/// (plan M3a Deviations 4 and 5).
#[derive(Debug, Clone)]
struct Seen {
    kind: EngineKind,
    text: String,
    caret: Pos,
}

/// A stand-in for a session buffer that is gone (a cell the key committed),
/// built from what the glue last saw, so the engine can leave it as it
/// leaves any buffer (plan M3a Deviation 4).
enum Ghost {
    Line(LineInput),
    Body(Box<EditorState>, BodyVisual),
}

impl Ghost {
    fn of(seen: &Seen) -> Self {
        match seen.kind {
            EngineKind::Line => {
                let mut input = LineInput::new(&seen.text);
                input.set_cursor(seen.caret.col);
                Ghost::Line(input)
            }
            EngineKind::Body => {
                let mut state = EditorState::new(Lines::from(seen.text.as_str()));
                state.cursor = Index2::new(seen.caret.row, seen.caret.col);
                Ghost::Body(Box::new(state), None)
            }
        }
    }

    fn buf(&mut self) -> FieldBuf<'_> {
        match self {
            Ghost::Line(input) => FieldBuf::Line(input),
            Ghost::Body(state, visual) => FieldBuf::Body(state, visual),
        }
    }
}

/// Runs `$call` with `$t` bound to the engine target for `$buf`: a
/// one-line buffer with the line state, or the body with its own.
macro_rules! target {
    ($glue:expr, $buf:expr, |$t:ident| $call:expr) => {
        match $buf {
            FieldBuf::Line(input) => {
                let mut b = OneLineBuf::new(input);
                let $t = Target { buf: &mut b, state: &mut $glue.line };
                $call
            }
            FieldBuf::Body(state, visual) => {
                let mut b = BodyBuf::new(state, visual);
                let $t = Target { buf: &mut b, state: &mut $glue.body };
                $call
            }
        }
    };
}

/// The vim profile's text engine and its field session (spec §3.3, §3.7).
#[derive(Debug, Default)]
pub struct Glue {
    engine: Engine,
    /// The open one-line field's state: fresh each time one opens (§3.11).
    line: BufState,
    /// The body's: kept while the same request stays open (§3.11).
    body: BufState,
    /// The request `body` belongs to; `None` before the first sync.
    body_owner: Option<Option<String>>,
    session: FieldSession,
    seen: Option<Seen>,
    /// The profile at the last sync, to tell a switch (spec §4.3).
    profile: Option<KeyProfile>,
    /// A mouse sweep is in flight in the session's buffer.
    sweeping: bool,
}

impl Glue {
    pub fn session(&self) -> FieldSession {
        self.session
    }

    pub fn set_session(&mut self, session: FieldSession) {
        self.session = session;
    }

    /// The field with a live session: it has the caret.
    pub fn live(&self) -> Option<FieldId> {
        match self.session {
            FieldSession::Live(id) => Some(id),
            _ => None,
        }
    }

    /// The field with a session, live or parked.
    pub fn owned(&self) -> Option<FieldId> {
        match self.session {
            FieldSession::Live(id) | FieldSession::Parked(id) => Some(id),
            FieldSession::None => None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.engine.mode()
    }

    pub fn view(&self) -> EngineView {
        EngineView { mode: self.engine.mode(), pending: self.engine.pending() }
    }

    /// The engine's half-typed command for the footer. A one-line field
    /// has no prompt row, so there the open search prompt's pattern follows
    /// its `/` or `?` (final review I-4); the body draws its own prompt.
    pub fn echo(&self) -> String {
        let echo = self.engine.echo();
        let line = matches!(self.session, FieldSession::Live(id) if id.engine_kind() == Some(EngineKind::Line));
        match self.engine.search_line() {
            Some(search) if line => echo + &search.text,
            _ => echo,
        }
    }

    /// Whether the engine is in idle Normal, but for a count (and a
    /// register, which undo ignores): a `u` or ctrl+r it hands back from
    /// here found nothing to step in the field, so it is the app history's
    /// (spec §3.7, repeated by the count). From Visual, Insert ctrl+o, or
    /// with an operator or prefix pending it is not (final review M-1).
    pub fn undo_ready(&self) -> bool {
        if self.engine.mode() != Mode::Normal {
            return false;
        }
        let echo = self.engine.echo();
        let mut rest = echo.chars();
        if echo.starts_with('"') {
            rest.next();
            rest.next();
        }
        rest.all(|c| c.is_ascii_digit())
    }

    fn state(&self, kind: EngineKind) -> &BufState {
        match kind {
            EngineKind::Line => &self.line,
            EngineKind::Body => &self.body,
        }
    }

    /// Whether the session's text changed since it opened (§3.11).
    pub fn edited(&self, kind: EngineKind) -> bool {
        self.state(kind).edited()
    }

    pub fn can_undo(&self, kind: EngineKind) -> bool {
        self.state(kind).can_undo()
    }

    pub fn can_redo(&self, kind: EngineKind) -> bool {
        self.state(kind).can_redo()
    }

    pub fn profile(&self) -> Option<KeyProfile> {
        self.profile
    }

    pub fn set_profile(&mut self, profile: KeyProfile) {
        self.profile = Some(profile);
    }

    pub fn sweeping(&self) -> bool {
        self.sweeping
    }

    /// A field got the caret (piece 3 §4.2 `enter`). A one-line field
    /// starts a fresh state; the body keeps its own (§3.11).
    pub fn enter(&mut self, buf: &mut FieldBuf<'_>, start: Start, seat: OpenSeat) {
        if let FieldBuf::Line(input) = buf {
            self.line = BufState::new();
            if seat == OpenSeat::After {
                let past = input.cursor() + 1;
                input.set_cursor(past);
            }
        }
        let seat = match seat {
            OpenSeat::Keep | OpenSeat::After => Seat::Keep,
            OpenSeat::End => Seat::End,
            OpenSeat::FirstNonBlank => Seat::FirstNonBlank,
        };
        target!(self, buf, |t| self.engine.enter(start, seat, t));
        self.sweeping = false;
        self.saw(buf);
    }

    /// The caret moved straight from the session's field to `to` in one
    /// event (piece 3 §4.2 `carry`): the mode carries. The source is left
    /// through a ghost of what the glue last saw (plan M3a Deviation 4).
    /// The ghost serves a source that is gone (the key that moved the
    /// caret committed its cell) and a one-line source, gone or not; the
    /// body, while its text is the one the glue saw, is left in place
    /// through [`Glue::carry_from`] (amendment C3).
    pub fn carry(&mut self, to: &mut FieldBuf<'_>) {
        let Some(seen) = self.seen.take() else {
            return self.enter(to, Start::Normal, OpenSeat::Keep);
        };
        let mut ghost = Ghost::of(&seen);
        self.carry_from(&mut ghost.buf(), to);
    }

    /// [`Glue::carry`] from a source that still exists, lent with `to`
    /// (amendment C3): the engine leaves the real text, so an unused
    /// autoindent goes from the body itself, and the body's history still
    /// fits its text at the next visit.
    pub fn carry_from(&mut self, from: &mut FieldBuf<'_>, to: &mut FieldBuf<'_>) {
        match (from, &mut *to) {
            (FieldBuf::Line(from), FieldBuf::Line(input)) => {
                let mut old = std::mem::take(&mut self.line);
                self.engine.carry(
                    Target { buf: &mut OneLineBuf::new(from), state: &mut old },
                    Target { buf: &mut OneLineBuf::new(input), state: &mut self.line },
                );
            }
            (FieldBuf::Line(from), FieldBuf::Body(state, visual)) => {
                let mut old = std::mem::take(&mut self.line);
                self.engine.carry(
                    Target { buf: &mut OneLineBuf::new(from), state: &mut old },
                    Target { buf: &mut BodyBuf::new(state, visual), state: &mut self.body },
                );
            }
            (FieldBuf::Body(from, paint), FieldBuf::Line(input)) => {
                self.line = BufState::new();
                self.engine.carry(
                    Target { buf: &mut BodyBuf::new(from, paint), state: &mut self.body },
                    Target { buf: &mut OneLineBuf::new(input), state: &mut self.line },
                );
            }
            // The body is one field: a carry never goes from it to itself.
            (FieldBuf::Body(..), FieldBuf::Body(..)) => {}
        }
        self.sweeping = false;
        self.saw(to);
    }

    /// The session's field lost the caret (piece 3 §4.2 `leave`). A
    /// one-line state ends with it (§3.11 `end_session`).
    pub fn leave(&mut self, buf: &mut FieldBuf<'_>) {
        target!(self, buf, |t| self.engine.leave(t));
        if matches!(buf, FieldBuf::Line(_)) {
            self.line = BufState::new();
        }
        self.seen = None;
        self.sweeping = false;
    }

    /// [`Glue::leave`] for a session buffer that is gone, or was replaced.
    pub fn leave_ghost(&mut self) {
        let Some(seen) = self.seen.take() else { return };
        let mut ghost = Ghost::of(&seen);
        self.leave(&mut ghost.buf());
    }

    /// After a click, a sweep or a key piece 4 handled (piece 3 §4.2).
    pub fn settle(&mut self, buf: &mut FieldBuf<'_>, how: Settled) {
        target!(self, buf, |t| self.engine.settle(t, how));
        self.sweeping = how == Settled::Sweep;
        self.saw(buf);
    }

    pub fn handle(&mut self, ev: KeyEvent, buf: &mut FieldBuf<'_>, view: &ViewCtx) -> Outcome {
        let out = target!(self, buf, |t| self.engine.handle(ev, t, view));
        self.saw(buf);
        out
    }

    /// Paste in Insert, Replace or the search prompt; `Declined` elsewhere.
    pub fn paste(&mut self, text: &str, buf: &mut FieldBuf<'_>) -> Outcome {
        let out = target!(self, buf, |t| self.engine.paste(text, t));
        self.saw(buf);
        out
    }

    /// The Visual selection in `buf`, the field with the caret, as a
    /// `[from, to)` range; `None` outside Visual (and while the search
    /// prompt is open over it). Charwise it is inclusive and takes the line
    /// break when it ends on a line's end with a line below, as the
    /// engine's operators take it. Linewise it is the whole lines but the
    /// last line break, so a replacement keeps that break (Vim's `c` on
    /// lines). It covers what the selection paints.
    pub fn visual_span(&self, buf: &mut FieldBuf<'_>) -> Option<(Pos, Pos)> {
        let Mode::Visual(shape) = self.engine.mode() else { return None };
        let anchor = self.engine.visual_anchor()?;
        Some(match buf {
            FieldBuf::Line(input) => span_of(&OneLineBuf::new(input), anchor, shape),
            FieldBuf::Body(state, visual) => span_of(&BodyBuf::new(state, visual), anchor, shape),
        })
    }

    /// The text of [`Glue::visual_span`], for ctrl+c and cmd+c (the
    /// user's decision, final review I-2).
    pub fn visual_text(&self, buf: &mut FieldBuf<'_>) -> Option<String> {
        let (from, to) = self.visual_span(buf)?;
        Some(match buf {
            FieldBuf::Line(input) => OneLineBuf::new(input).slice(from, to),
            FieldBuf::Body(state, visual) => BodyBuf::new(state, visual).slice(from, to),
        })
    }

    /// Whether the text changed.
    pub fn edit(&mut self, buf: &mut FieldBuf<'_>, edit: &Edit<'_>) -> bool {
        let changed = target!(self, buf, |t| self.engine.external_edit(t, |s| edit.apply(s)));
        self.saw(buf);
        changed
    }

    /// The session's text was replaced outside the engine: start over on
    /// it in the same mode, with a fresh state (plan M3a Deviation 5). The
    /// replaced buffer is not left first: its positions no longer fit.
    /// Insert ctrl+o was typing too, so it re-enters as Insert, as Insert
    /// and Replace do (the user's ruling, final review).
    pub fn rebase(&mut self, buf: &mut FieldBuf<'_>) {
        let start = match self.engine.mode() {
            Mode::Insert | Mode::Replace | Mode::InsertNormal { .. } => Start::Insert,
            _ => Start::Normal,
        };
        if matches!(buf, FieldBuf::Body(..)) {
            self.body = BufState::new();
        }
        self.enter(buf, start, OpenSeat::Keep);
    }

    /// Whether `buf` holds other text than the glue last saw there.
    pub fn differs(&self, buf: &FieldBuf<'_>) -> bool {
        self.seen.as_ref().is_some_and(|s| s.text != buf.text())
    }

    /// The open request: a switch drops the body's state and clears the
    /// search highlight (plan M3a Deviation 19, spec §3.14).
    pub fn own_body(&mut self, slug: Option<&str>) {
        let owner = slug.map(str::to_string);
        if self.body_owner.as_ref() == Some(&owner) {
            return;
        }
        if self.body_owner.is_some() {
            self.engine.no_hlsearch();
        }
        self.body = BufState::new();
        self.body_owner = Some(owner);
    }

    /// The open request's slug changed in place from `from` to `to` (a
    /// rename, a space rename, their undo or redo, a scratch saved under a
    /// name): the body's state follows it, its history and the search
    /// highlight kept (amendment C2). Only a state owned by `from` moves:
    /// in arrows the owner may name a request closed since, whose state
    /// must not be adopted. A request switch still goes through
    /// [`Glue::own_body`].
    pub fn follow_body(&mut self, from: Option<&str>, to: Option<&str>) {
        if self.body_owner == Some(from.map(str::to_string)) {
            self.body_owner = Some(to.map(str::to_string));
        }
    }

    /// The app history took the live session's edits as a step
    /// (`App::flush_field_session`): the text it holds is the new start.
    /// The line state is fresh at every enter, so it always ends. The body
    /// state ends only when the body owns the session: its history outlives
    /// closes, and an app op flushing the URL must not wipe it.
    pub fn flush(&mut self) {
        self.line.end_session();
        if self.owned() == Some(FieldId::Body) {
            self.body.end_session();
        }
    }

    /// What the body draws for the engine's search (spec §5.6). The prompt
    /// only shows while the body has the session.
    pub fn body_search(&self, state: &mut EditorState, visual: &mut BodyVisual, rows: std::ops::Range<usize>) -> BodySearch {
        let b = BodyBuf::new(state, visual);
        let prompt = (self.session == FieldSession::Live(FieldId::Body)).then(|| self.engine.search_line()).flatten();
        let current = prompt.as_ref().and_then(|_| self.engine.search_preview(&b));
        BodySearch { prompt, matches: self.engine.search_matches(&b, rows), current }
    }

    fn saw(&mut self, buf: &FieldBuf<'_>) {
        self.seen = Some(Seen { kind: buf.kind(), text: buf.text(), caret: buf.caret() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyroute::notation;

    fn keys(g: &mut Glue, buf: &mut FieldBuf<'_>, typed: &str) {
        for ev in notation::events(typed) {
            g.handle(ev, buf, &ViewCtx::default());
        }
    }

    fn opened(text: &str, start: Start, seat: OpenSeat, caret: usize) -> (Glue, LineInput) {
        let mut g = Glue::default();
        let mut input = LineInput::new(text);
        input.set_cursor(caret);
        g.enter(&mut FieldBuf::Line(&mut input), start, seat);
        (g, input)
    }

    #[test]
    fn a_normal_open_clamps_and_keys_edit_through_the_engine() {
        let (mut g, mut input) = opened("abc", Start::Normal, OpenSeat::Keep, 3);
        assert_eq!((g.mode(), input.cursor()), (Mode::Normal, 2), "a Normal caret rests on a char");
        keys(&mut g, &mut FieldBuf::Line(&mut input), "x");
        assert_eq!(input.text(), "ab");
        assert!(g.edited(EngineKind::Line) && g.can_undo(EngineKind::Line));
        keys(&mut g, &mut FieldBuf::Line(&mut input), "u");
        assert_eq!(input.text(), "abc");
    }

    /// Plan M3a Deviation 12: the seats an opening key asks for.
    #[test]
    fn insert_seats() {
        for (seat, caret, want) in [
            (OpenSeat::Keep, 1, 1),
            (OpenSeat::After, 1, 2),
            (OpenSeat::After, 4, 4),
            (OpenSeat::End, 1, 4),
            (OpenSeat::FirstNonBlank, 0, 2),
        ] {
            let (g, input) = opened("  ab", Start::Insert, seat, caret);
            assert_eq!((g.mode(), input.cursor()), (Mode::Insert, want), "{seat:?} from {caret}");
        }
    }

    /// Plan M3a Deviation 4: Tab committed the source cell, so its buffer is
    /// gone; the mode carries into the next one, which takes the typing.
    #[test]
    fn a_carry_from_a_gone_buffer_keeps_insert() {
        let (mut g, mut first) = opened("", Start::Insert, OpenSeat::Keep, 0);
        keys(&mut g, &mut FieldBuf::Line(&mut first), "ab");
        drop(first);
        let mut next = LineInput::new("");
        g.carry(&mut FieldBuf::Line(&mut next));
        assert_eq!(g.mode(), Mode::Insert);
        assert!(!g.edited(EngineKind::Line), "the next field starts its own session");
        keys(&mut g, &mut FieldBuf::Line(&mut next), "c");
        assert_eq!(next.text(), "c");
    }

    #[test]
    fn a_carry_from_the_body_keeps_its_state_for_the_next_visit() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("one\ntwo"));
        let mut visual: BodyVisual = None;
        g.own_body(Some("main/a"));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "x");
        let mut url = LineInput::new("u");
        g.carry(&mut FieldBuf::Line(&mut url));
        assert_eq!(g.mode(), Mode::Normal);
        assert!(g.can_undo(EngineKind::Body), "the body keeps its history while its request is open");
        g.leave(&mut FieldBuf::Line(&mut url));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "u");
        assert_eq!(body.lines.to_string(), "one\ntwo");
    }

    #[test]
    fn leaving_a_gone_buffer_ends_insert() {
        let (mut g, mut input) = opened("", Start::Insert, OpenSeat::Keep, 0);
        keys(&mut g, &mut FieldBuf::Line(&mut input), "ab");
        drop(input);
        g.leave_ghost();
        assert_eq!(g.mode(), Mode::Normal);
        assert!(!g.edited(EngineKind::Line), "a one-line state ends with its session");
    }

    /// Plan M3a Deviation 5: text replaced under a session drops the history
    /// and keeps the mode.
    #[test]
    fn a_rebase_drops_the_history_and_keeps_the_mode() {
        let (mut g, mut input) = opened("abc", Start::Insert, OpenSeat::Keep, 3);
        keys(&mut g, &mut FieldBuf::Line(&mut input), "d");
        let mut replaced = LineInput::new("zz");
        assert!(g.differs(&FieldBuf::Line(&mut replaced)));
        g.rebase(&mut FieldBuf::Line(&mut replaced));
        assert_eq!(g.mode(), Mode::Insert);
        assert!(!g.can_undo(EngineKind::Line));
        assert!(!g.differs(&FieldBuf::Line(&mut replaced)));
    }

    /// Plan M3a Deviation 19.
    #[test]
    fn the_body_state_follows_the_open_request() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("abc"));
        let mut visual: BodyVisual = None;
        g.own_body(Some("main/a"));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "x");
        g.own_body(Some("main/a"));
        assert!(g.can_undo(EngineKind::Body), "the same request keeps it");
        g.own_body(Some("main/b"));
        assert!(!g.can_undo(EngineKind::Body), "a switch drops it");
    }

    /// Amendment C2: a slug changed in place (a rename) keeps the state.
    #[test]
    fn the_body_state_follows_a_rename() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("abc"));
        let mut visual: BodyVisual = None;
        g.own_body(Some("main/a"));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "x");
        g.follow_body(Some("main/other"), Some("main/elsewhere"));
        g.own_body(Some("main/a"));
        assert!(g.can_undo(EngineKind::Body), "a rename of another request moves nothing");
        g.follow_body(Some("main/a"), Some("main/renamed"));
        g.own_body(Some("main/renamed"));
        assert!(g.can_undo(EngineKind::Body), "the renamed request keeps it");
    }

    /// Amendment C3: a carry out of the real body leaves it there, so an
    /// unused autoindent goes from its text and its history still fits.
    #[test]
    fn a_carry_from_the_real_body_strips_its_autoindent() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("  a"));
        let mut visual: BodyVisual = None;
        g.own_body(Some("main/a"));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "o");
        assert_eq!(body.lines.to_string(), "  a\n  ");
        let mut url = LineInput::new("u");
        g.carry_from(&mut FieldBuf::Body(&mut body, &mut visual), &mut FieldBuf::Line(&mut url));
        assert_eq!((g.mode(), body.lines.to_string()), (Mode::Insert, "  a\n".to_string()));
        g.leave(&mut FieldBuf::Line(&mut url));
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "u");
        assert_eq!(body.lines.to_string(), "  a", "the history survived");
    }

    /// Plan M3a Deviation 13: Vim's bracketed paste outside Insert.
    #[test]
    fn an_append_goes_after_the_caret_or_before_column_zero() {
        let (mut g, mut input) = opened("ab", Start::Normal, OpenSeat::Keep, 0);
        g.edit(&mut FieldBuf::Line(&mut input), &Edit::Append("XY"));
        assert_eq!((input.text(), input.cursor()), ("XYab", 1));
        let (mut g, mut input) = opened("ab", Start::Normal, OpenSeat::Keep, 1);
        g.edit(&mut FieldBuf::Line(&mut input), &Edit::Append("XY"));
        assert_eq!((input.text(), input.cursor()), ("abXY", 3));
        keys(&mut g, &mut FieldBuf::Line(&mut input), "u");
        assert_eq!(input.text(), "ab", "one undo step");
    }

    #[test]
    fn a_whole_replace_keeps_the_caret() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("{\"a\":1}"));
        let mut visual: BodyVisual = None;
        g.own_body(None);
        body.cursor = edtui::Index2::new(0, 3);
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        g.edit(&mut FieldBuf::Body(&mut body, &mut visual), &Edit::Whole("{\n  \"a\": 1\n}"));
        assert_eq!(body.lines.to_string(), "{\n  \"a\": 1\n}");
        assert_eq!((body.cursor.row, body.cursor.col), (0, 0), "row 0 has one char: the caret clamps");
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "u");
        assert_eq!(body.lines.to_string(), "{\"a\":1}");
    }

    #[test]
    fn at_caret_types_where_insert_would() {
        let (mut g, mut input) = opened("ab", Start::Insert, OpenSeat::Keep, 1);
        g.edit(&mut FieldBuf::Line(&mut input), &Edit::AtCaret("{{v}}"));
        assert_eq!((input.text(), input.cursor()), ("a{{v}}b", 6));
        assert_eq!(g.mode(), Mode::Insert);
    }

    #[test]
    fn the_edges() {
        let (u, c) = (FieldId::Url, FieldId::TableCell { row: 0, col: 1 });
        assert_eq!(edge(None, None), Edge::Idle);
        assert_eq!(edge(None, Some(u)), Edge::Open(u));
        assert_eq!(edge(Some(u), None), Edge::Close(u));
        assert_eq!(edge(Some(u), Some(u)), Edge::Same(u));
        assert_eq!(edge(Some(u), Some(c)), Edge::Carry(u, c));
    }

    #[test]
    fn a_body_search_reports_its_prompt_and_matches() {
        let mut g = Glue::default();
        let mut body = EditorState::new(Lines::from("foo bar foo"));
        let mut visual: BodyVisual = None;
        g.own_body(None);
        g.enter(&mut FieldBuf::Body(&mut body, &mut visual), Start::Normal, OpenSeat::Keep);
        g.set_session(FieldSession::Live(FieldId::Body));
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "/foo");
        let s = g.body_search(&mut body, &mut visual, 0..usize::MAX);
        assert_eq!(s.prompt.as_ref().map(|p| p.text.as_str()), Some("foo"));
        assert_eq!(s.current, Some((Pos::new(0, 8), Pos::new(0, 11))), "incsearch: the match Enter would land on");
        keys(&mut g, &mut FieldBuf::Body(&mut body, &mut visual), "<CR>");
        let s = g.body_search(&mut body, &mut visual, 0..usize::MAX);
        assert_eq!((s.prompt, s.current), (None, None));
        assert_eq!(s.matches, vec![(Pos::new(0, 0), Pos::new(0, 3)), (Pos::new(0, 8), Pos::new(0, 11))]);
    }

    /// Spec §3.7: piece 4 calls the engine here and nowhere else.
    #[test]
    fn only_the_glue_calls_the_engine() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let names = ["Engine", "BufState", "OneLineBuf", "BodyBuf", "Target", "Splicer"];
        let mut stack = vec![src.clone()];
        let mut hits = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
                if path.is_dir() {
                    if rel != "vim" {
                        stack.push(path);
                    }
                    continue;
                }
                if rel == "keyroute/engine_glue.rs" || !rel.ends_with(".rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for name in names {
                    let needle = format!("engine::{name}");
                    let braced = text.contains("vim::engine::{") && text.contains(&format!("{name},"));
                    if text.contains(&needle) || braced {
                        hits.push(format!("{rel}: {name}"));
                    }
                }
            }
        }
        assert!(hits.is_empty(), "the engine is called outside engine_glue: {hits:?}");
    }
}
