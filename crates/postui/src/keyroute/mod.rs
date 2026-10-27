//! The key router (piece 4 spec §3): where a key lands, what a key means
//! there, and what is half-typed. `App` builds the context once per key
//! (`app/router.rs`); everything in this module is pure.

pub mod ctx;
pub mod engine_glue;
pub mod ex;
#[cfg(test)]
pub mod keylist;
pub mod notation;
pub mod panes;
pub mod pending;
pub mod spell;
pub mod table;

/// The most a count can say: the router's cap, which is the engine's.
pub use crate::vim::engine::settings::MAX_COUNT;
pub use ctx::{EngineKind, EngineView, FieldCaret, FieldId, FieldSpell, KeyCtx, SpellCtx, Surface};
/// How a step asks a field to open: part of the router's `Step`.
pub use engine_glue::OpenAs;

use crate::action::{Action, TabTo};
use crate::config::KeyProfile;
use notation::Tok;
use pending::Pending;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use table::{CountRule, Intent, Match, Named, Surfaces};

/// One thing a resolved key does (spec §3.1).
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A named key through today's routing (`App::handle_key_routed`).
    Key(KeyEvent),
    Action(Action),
    /// A key for the engine on the live field (spec §3.7).
    Engine(KeyEvent),
    /// If this event opens a field, open it so (spec §3.1; plan M3a
    /// Deviation 3). Applied by the end-of-event sync.
    OpenAs(engine_glue::OpenAs),
    /// Bring the engine session in line with the caret now, not at the
    /// end of the event: the engine keys after it reach the field a key
    /// before it opened (`Intent::Edit`).
    Sync,
}

/// What the router made of a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Route the key as main does today (spec §3.6).
    Declined,
    /// The key was taken: the pending keys grew, or vim swallowed it.
    Consumed,
    Run(Vec<Step>),
}

/// A list change a vim key made (spec §5.12; plan M5b Deviation 7): the
/// row's keys and the count typed with them, for `.` to replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub keys: &'static str,
    pub count: Option<usize>,
}

/// The one pending machine (spec §3.4).
#[derive(Debug, Default)]
pub struct Router {
    pending: Pending,
    /// The surface the pending keys were typed on.
    surface: Option<Surface>,
    /// `q{reg}` was typed (spec §5.11): the footer says so until the next
    /// bare `q` in Normal, which is swallowed and clears it.
    recording: Option<char>,
    /// The list change the last key ran (`take_change`): the app keeps it
    /// for `.` once the event shows it changed something.
    change: Option<Change>,
}

impl Router {
    /// Drops whatever is pending, the recording flag included: a profile
    /// switch, a mouse press.
    pub fn clear(&mut self) {
        self.clear_pending();
        self.recording = None;
    }

    /// Drops the pending keys and keeps the recording flag: a field's
    /// close (amendment A4-1; the final review's ruling I-4, so recording
    /// a field edit still ends at the next bare `q`).
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// The register `q` was typed with, while the fake recording is on.
    pub fn recording(&self) -> Option<char> {
        self.recording
    }

    /// The list change the last key ran, once (plan M5b Deviation 7).
    pub fn take_change(&mut self) -> Option<Change> {
        self.change.take()
    }

    /// The closing `q` of `q{reg}…q` (spec §5.11): a bare `q` with nothing
    /// pending while the flag is on is swallowed and clears it.
    fn stops_recording(&mut self, tok: Tok) -> bool {
        if self.recording.is_some() && self.pending.is_idle() && tok == Tok::Char('q') {
            self.recording = None;
            return true;
        }
        false
    }

    /// A complete row: arms the recording flag for `q{reg}`, then expands.
    fn run_row(&mut self, b: &'static table::Binding, buf: &[Tok], cap: usize) -> Resolved {
        let count = self.pending.count();
        if matches!(b.intent, Intent::Recording)
            && let Some(Tok::Char(c)) = buf.last()
            && (c.is_ascii_alphanumeric() || *c == '"')
        {
            self.recording = Some(*c);
        }
        // Spec §5.12: vim's `dd`, `p` and `P` are what `.` repeats.
        if b.profile == table::Profiles::VIM
            && matches!(
                b.intent,
                Intent::Verb(crate::action::Verb::Delete { .. } | crate::action::Verb::Put { .. })
            )
        {
            self.change = Some(Change { keys: b.keys, count });
        }
        self.pending.clear();
        match b.intent {
            Intent::Swallow => Resolved::Consumed,
            _ => Resolved::Run(expand(b, count, cap)),
        }
    }

    /// The footer's echo of what is pending (spec §3.8).
    pub fn echo(&self) -> String {
        self.pending.echo()
    }

    /// The surface the pending keys were typed on.
    pub fn typed_on(&self) -> Option<Surface> {
        self.surface
    }

    /// Resolves one key against the pending state and the context: the
    /// router's whole job (spec §3.4). Declined keys run today's routing.
    pub fn resolve(&mut self, ev: KeyEvent, ctx: &KeyCtx) -> Resolved {
        // A key typed on one surface never completes a sequence on another.
        if self.surface != Some(ctx.surface) {
            self.pending.clear();
            self.surface = Some(ctx.surface);
        }
        if let FieldCaret::Live(id) = ctx.field {
            // A field the engine runs: the lookahead (spec §3.7).
            if ctx.profile == KeyProfile::Vim
                && let Some(engine) = ctx.engine
            {
                return self.resolve_field(ev, table::field_class(id, ctx.surface), engine, ctx);
            }
            // Arrows, or a vim field whose session the sync has not opened
            // (a dialog's, until plan M3b Task 3): the field types every key.
            self.pending.clear();
            return Resolved::Declined;
        }
        // An alt chord or a modified named key: no row names it, so what
        // was pending is dropped and the key goes on (`g` then ctrl+c
        // still reaches the quit).
        let Some(tok) = Tok::of(&ev) else {
            self.pending.clear();
            return Resolved::Declined;
        };
        self.step(ev, tok, ctx, true)
    }

    /// A key in a field the engine runs (spec §3.7). In Insert, Visual, the
    /// search prompt, or with an engine command half-typed, the engine sees
    /// it first. In idle Normal the router looks first, at the field rows
    /// only, and replays whatever matches none of them into the engine as
    /// typed: the engine never knows the router looked.
    fn resolve_field(
        &mut self,
        ev: KeyEvent,
        class: Surfaces,
        engine: EngineView,
        ctx: &KeyCtx,
    ) -> Resolved {
        if engine.mode != engine_glue::Mode::Normal || engine.pending {
            self.pending.clear();
            return Resolved::Run(vec![Step::Engine(ev)]);
        }
        // An alt chord or a modified named key: no field row names it.
        let Some(tok) = Tok::of(&ev) else {
            return self.replay(ev);
        };
        if tok == Tok::Named(KeyCode::Esc) && !self.pending.is_idle() {
            self.pending.clear();
            return Resolved::Consumed;
        }
        if self.stops_recording(tok) {
            return Resolved::Consumed;
        }
        if let Tok::Char(c) = tok
            && self.pending.digit(c)
        {
            return Resolved::Consumed;
        }
        let mut buf = self.pending.keys().to_vec();
        buf.push(tok);
        match table::lookup_class(&buf, ctx.profile, class) {
            Match::Complete(b) => self.run_row(b, &buf, ctx.cap),
            Match::Prefix => {
                self.pending.push(tok);
                Resolved::Consumed
            }
            Match::NoMatch => self.replay(ev),
        }
    }

    /// What the router held, then `ev`, into the engine in order.
    fn replay(&mut self, ev: KeyEvent) -> Resolved {
        let mut steps: Vec<Step> = self.pending.replay().into_iter().map(Step::Engine).collect();
        steps.push(Step::Engine(ev));
        self.pending.clear();
        Resolved::Run(steps)
    }

    fn step(&mut self, ev: KeyEvent, tok: Tok, ctx: &KeyCtx, first: bool) -> Resolved {
        // Spec §5.10 (plan M3b Deviation 9): a dialog's own letters answer
        // it, ahead of any row, in either case as the dialog matches them.
        if ctx.surface == Surface::Dialog
            && let Tok::Char(c) = tok
            && ctx.answers.iter().any(|a| a.eq_ignore_ascii_case(&c))
        {
            self.pending.clear();
            return Resolved::Declined;
        }
        // 1. Esc clears what is pending, silently.
        if tok == Tok::Named(KeyCode::Esc) && !self.pending.is_idle() {
            self.pending.clear();
            return Resolved::Consumed;
        }
        if self.stops_recording(tok) {
            return Resolved::Consumed;
        }
        if ctx.profile == KeyProfile::Vim {
            // 2. Digits build the count, where some row takes one (plan M1
            // Deviation 22). `takes_count` reads the vim rows, so it is
            // never asked for the arrows profile.
            if let Tok::Char(c) = tok
                && table::takes_count(ctx.surface)
                && self.pending.digit(c)
            {
                return Resolved::Consumed;
            }
        }
        // keys.toml's bare chars win while nothing is pending, in both
        // profiles (§4.4, plan M2 Deviation 18).
        if self.pending.is_idle()
            && matches!(tok, Tok::Char(_))
            && let Some(a) = &ctx.user_bare
        {
            return Resolved::Run(vec![Step::Action(a.clone())]);
        }
        // 3. The table.
        let mut buf = self.pending.keys().to_vec();
        buf.push(tok);
        match table::lookup(&buf, ctx.profile, ctx.surface) {
            Match::Complete(b) => self.run_row(b, &buf, ctx.cap),
            Match::Prefix => {
                self.pending.push(tok);
                Resolved::Consumed
            }
            Match::NoMatch => self.no_match(ev, tok, ctx, first),
        }
    }

    /// 4. Nothing on this surface starts with these keys.
    fn no_match(&mut self, ev: KeyEvent, tok: Tok, ctx: &KeyCtx, first: bool) -> Resolved {
        if self.pending.is_idle() {
            // In vim a bare char on a navigation surface, or in the
            // response's V, is vim's beep: no component letter is left to
            // reach (spec §3.4, plan M1 Deviation 1). Space is the
            // surface's own key.
            let swallow = ctx.profile == KeyProfile::Vim
                && matches!(tok, Tok::Char(c) if c != ' ')
                && Surfaces::KEYED.has(ctx.surface);
            return if swallow { Resolved::Consumed } else { Resolved::Declined };
        }
        if self.pending.keys().is_empty() {
            // Only a count: an arrow or a page key repeats (`3<Down>`);
            // any other key starts afresh, so a stray count never
            // multiplies a later key.
            let n = self.pending.count().unwrap_or(1);
            self.pending.clear();
            if matches!(
                tok,
                Tok::Named(
                    KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                )
            ) {
                return Resolved::Run(vec![Step::Key(ev); n.min(ctx.cap)]);
            }
            return if first { self.step(ev, tok, ctx, false) } else { Resolved::Declined };
        }
        // A prefix and a key that does not extend it: vim's beep. A ctrl
        // chord is the exception: it goes on (`g` then ctrl+c reaches the
        // quit).
        self.pending.clear();
        if matches!(tok, Tok::Ctrl(_)) { Resolved::Declined } else { Resolved::Consumed }
    }
}

/// A complete row's steps under its count rule.
pub(crate) fn expand(b: &table::Binding, count: Option<usize>, cap: usize) -> Vec<Step> {
    // A refused count runs nothing but its note (spec §3.5). A count of
    // one is no count, as in vim: `1yy` is `yy`.
    if let (CountRule::Refuse(note), Some(2..)) = (b.count, count) {
        return vec![note_step(note)];
    }
    let times = |limit: usize| match b.count {
        CountRule::Repeat => count.unwrap_or(1).min(limit),
        CountRule::Ignore | CountRule::Pass | CountRule::Refuse(_) => 1,
    };
    match b.intent {
        Intent::Key(k) => vec![Step::Key(k.event()); times(cap)],
        Intent::Edge { bare, home } => match count {
            None => vec![Step::Key(bare.event())],
            Some(n) => {
                let mut steps = vec![Step::Key(home.event())];
                let down = Named::key(KeyCode::Down).event();
                steps.extend(std::iter::repeat_n(Step::Key(down), n.saturating_sub(1).min(cap)));
                steps
            }
        },
        // An action's count is not the surface's rows: `5u` undoes five.
        Intent::Act(f) => vec![Step::Action(f()); times(usize::MAX)],
        // `{N}dd` and `{N}p` hand N to the verb, which runs it as one
        // undo step; a put's N is capped by the surface like a motion's
        // (plan M2 Deviation 10), so `9999p` can't write 9 999 files.
        // An add verb opens what it makes in Insert (spec §5.6): the app's
        // `OnSelection` arm says so, for this row and the `:` line alike.
        Intent::Verb(v) => {
            if b.count == CountRule::Pass {
                let n = count.unwrap_or(1);
                let n = if matches!(v, crate::action::Verb::Put { .. }) { n.min(cap) } else { n };
                vec![Step::Action(Action::OnSelection(v.with_count(n)))]
            } else {
                vec![Step::Action(Action::OnSelection(v)); times(cap)]
            }
        }
        Intent::Open { key, seat } => {
            vec![Step::OpenAs(engine_glue::OpenAs::Insert(seat)), Step::Key(key.event())]
        }
        // The engine reads the count as Vim does: `2cc` on one line.
        Intent::Edit { key, keys } => {
            let mut steps =
                vec![Step::OpenAs(engine_glue::OpenAs::Normal), Step::Key(key.event()), Step::Sync];
            let typed = count.map_or_else(|| keys.to_string(), |n| format!("{n}{keys}"));
            steps.extend(notation::parse(&typed).into_iter().map(|t| Step::Engine(t.event())));
            steps
        }
        Intent::Note(text) => vec![note_step(text)],
        // The field closes (its own close, keeping the text), then the key
        // runs on the closed surface (plan M3b Deviation 7). A dialog's
        // field closes to selected through `CloseField`, never its Esc,
        // which cancels a one-field prompt.
        Intent::Leave(k) => {
            let mut steps = vec![Step::Action(Action::CloseField)];
            steps.extend(std::iter::repeat_n(Step::Key(k.event()), times(cap)));
            steps
        }
        Intent::Reopen(seat) => vec![
            Step::OpenAs(seat.map_or(engine_glue::OpenAs::Normal, engine_glue::OpenAs::Insert)),
            Step::Action(Action::ReopenField),
        ],
        // Spec §5.8 (plan M4a Deviations 7 and 8). A refused key, or one
        // onto the tab already showing, leaves the field open: the app
        // closes it only when there is a tab to go to (`App::go_tab`).
        Intent::Tab { delta } => {
            let to = match count {
                Some(n) if delta > 0 => TabTo::Nth(n),
                Some(n) => TabTo::By(-i64::try_from(n).unwrap_or(i64::MAX)),
                None => TabTo::By(i64::from(delta)),
            };
            vec![Step::Action(Action::GoTab { to })]
        }
        // As `{N}gt`: the app closes an open field only when there is a pane
        // to move to (the user's decision, 2026-10-05).
        Intent::Pane(dir) => vec![Step::Action(Action::PaneMove(dir))],
        Intent::Recording => vec![note_step(table::NO_MACROS)],
        Intent::CloseThen(f) => vec![Step::Action(Action::CloseField), Step::Action(f())],
        // Plan M5a Task 3: the motion reads the count itself.
        Intent::Motion(f) => vec![Step::Action(Action::ResponseMove(f(count)))],
        Intent::Extend(f) => vec![Step::Action(Action::ResponseExtend(f(count)))],
        Intent::Counted(f) => vec![Step::Action(f(count))],
        Intent::Swallow => Vec::new(),
    }
}

/// `.` (spec §5.12; plan M5b Deviation 2): the steps `change`'s keys run on
/// `surface` with its count, as if typed there, the surface's own count
/// rule and cap included. `None` where no vim list-verb row takes them
/// (the response, a Manage detail pane).
pub(crate) fn replay(change: Change, surface: Surface, cap: usize) -> Option<Vec<Step>> {
    match table::lookup(&notation::parse(change.keys), KeyProfile::Vim, surface) {
        Match::Complete(b) if matches!(b.intent, Intent::Verb(_)) => {
            Some(expand(b, change.count, cap))
        }
        _ => None,
    }
}

/// A short note: an Info toast (plan M1 Deviation 9).
fn note_step(text: &str) -> Step {
    Step::Action(Action::ShowToast(text.to_string(), crate::components::toast::ToastKind::Info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::toast::ToastKind;
    use crate::config::KeyProfile::{Arrows, Vim};
    use crate::vim::engine::settings::MAX_COUNT;
    use ratatui::crossterm::event::KeyModifiers;

    fn cx(profile: KeyProfile, surface: Surface) -> KeyCtx {
        KeyCtx {
            profile,
            surface,
            field: FieldCaret::None,
            user_bare: None,
            cap: MAX_COUNT,
            engine: None,
            answers: Vec::new(),
        }
    }

    use crate::keyroute::engine_glue::{Mode, OpenAs, OpenSeat};

    /// An open URL line with an engine session in `mode`.
    fn in_url(mode: Mode, pending: bool) -> KeyCtx {
        KeyCtx {
            field: FieldCaret::Live(FieldId::Url),
            engine: Some(EngineView { mode, pending }),
            ..cx(Vim, Surface::UrlLine)
        }
    }

    fn eng(keys: &str) -> Vec<Step> {
        notation::events(keys).into_iter().map(Step::Engine).collect()
    }

    /// Spec §3.7: in Insert, Visual, the search prompt or with a command
    /// half-typed, the engine sees every key first.
    #[test]
    fn a_busy_engine_sees_every_key() {
        for (mode, pending) in [
            (Mode::Insert, false),
            (Mode::Visual(crate::keyroute::engine_glue::Shape::Char), false),
            (Mode::Normal, true),
        ] {
            let s = in_url(mode, pending);
            for keys in ["j", ":", "q", "3", "<Esc>", "<C-d>"] {
                assert_eq!(
                    feed(&mut Router::default(), &s, keys),
                    run(eng(keys)),
                    "{mode:?} {pending}: {keys}"
                );
            }
        }
    }

    /// Spec §3.7: idle Normal looks at the field rows first and replays
    /// anything else into the engine, count and prefix included.
    #[test]
    fn idle_normal_looks_ahead_then_replays() {
        let s = in_url(Mode::Normal, false);
        assert_eq!(feed(&mut Router::default(), &s, "x"), run(eng("x")));
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "2"), Resolved::Consumed);
        assert_eq!(r.echo(), "2");
        assert_eq!(feed(&mut r, &s, "d"), run(eng("2d")), "the count reaches the engine as typed");
        assert_eq!(
            feed(&mut Router::default(), &s, "0"),
            run(eng("0")),
            "a leading 0 is the motion"
        );
        assert_eq!(feed(&mut Router::default(), &s, "12<C-a>"), run(eng("12<C-a>")));
        assert_eq!(
            feed(&mut Router::default(), &s, "<Esc>"),
            run(eng("<Esc>")),
            "an idle Esc is the engine's to decline"
        );
        let mut r = Router::default();
        feed(&mut r, &s, "3");
        assert_eq!(feed(&mut r, &s, "<Esc>"), Resolved::Consumed, "Esc drops the router's count");
        assert_eq!(r.echo(), "");
    }

    /// Spec §3.7, §5.6 (the user's decision 2): `j` and `k` leave a one-line
    /// field and move; the count repeats the move.
    #[test]
    fn j_and_k_leave_a_one_line_field() {
        let s = in_url(Mode::Normal, false);
        let close = Step::Action(Action::CloseField);
        assert_eq!(
            feed(&mut Router::default(), &s, "j"),
            run(vec![close.clone(), key(KeyCode::Down)])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "3k"),
            run(vec![close.clone(), key(KeyCode::Up), key(KeyCode::Up), key(KeyCode::Up)])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "<C-d>"),
            run(vec![close, ctrl_key(KeyCode::Char('d'))])
        );
        let body = KeyCtx { field: FieldCaret::Live(FieldId::Body), surface: Surface::Body, ..s };
        assert_eq!(feed(&mut Router::default(), &body, "j"), run(eng("j")), "a body motion");
    }

    /// Spec §5.10, plan M3b Deviation 9: a dialog's letters answer it; the
    /// moves are rows only where they do not.
    #[test]
    fn a_dialogs_answer_letters_beat_its_moves() {
        let dialog = KeyCtx { answers: vec!['y', 'n'], ..cx(Vim, Surface::Dialog) };
        assert_eq!(feed(&mut Router::default(), &dialog, "y"), Resolved::Declined, "y answers");
        assert_eq!(feed(&mut Router::default(), &dialog, "j"), run(vec![key(KeyCode::Down)]));
        assert_eq!(feed(&mut Router::default(), &dialog, "G"), run(vec![key(KeyCode::End)]));
        let j_answers = KeyCtx { answers: vec!['j'], ..cx(Vim, Surface::Dialog) };
        assert_eq!(
            feed(&mut Router::default(), &j_answers, "j"),
            Resolved::Declined,
            "an answer is never a move"
        );
        assert_eq!(
            feed(&mut Router::default(), &j_answers, "J"),
            Resolved::Declined,
            "either case, as the dialog matches"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Dialog), "j"),
            Resolved::Declined,
            "arrows has no vim rows"
        );
    }

    /// Plan M3b Deviations 7 and 8: a one-field prompt's field in idle
    /// Normal confirms on Enter and cancels on Esc; `j` closes it, then moves.
    #[test]
    fn a_prompt_field_confirms_cancels_and_leaves() {
        let s = KeyCtx {
            field: FieldCaret::Live(FieldId::ModalInput(0)),
            engine: Some(EngineView { mode: Mode::Normal, pending: false }),
            ..cx(Vim, Surface::Prompt)
        };
        assert_eq!(
            feed(&mut Router::default(), &s, "<CR>"),
            run(vec![Step::Action(Action::ConfirmTopModal)])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "<Esc>"),
            run(vec![Step::Action(Action::CancelTopModal)])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "j"),
            run(vec![Step::Action(Action::CloseField), key(KeyCode::Down)])
        );
        let form = KeyCtx { surface: Surface::Form, ..s.clone() };
        assert_eq!(
            feed(&mut Router::default(), &form, "<Esc>"),
            run(eng("<Esc>")),
            "a form field's idle Esc is the engine's to decline"
        );
        let insert =
            KeyCtx { engine: Some(EngineView { mode: Mode::Insert, pending: false }), ..s };
        assert_eq!(
            feed(&mut Router::default(), &insert, "<Esc>"),
            run(eng("<Esc>")),
            "Insert's Esc is the engine's"
        );
    }

    /// Spec §3.7 item 2: a query box has no Normal, so every key is the
    /// engine's, the letters the field rows name included.
    #[test]
    fn a_query_box_sends_every_key_to_the_engine() {
        let s = KeyCtx {
            field: FieldCaret::Live(FieldId::Palette),
            engine: Some(EngineView { mode: Mode::Insert, pending: false }),
            ..cx(Vim, Surface::Picker)
        };
        for keys in ["j", ":", "q", "<Esc>", "<C-n>"] {
            assert_eq!(feed(&mut Router::default(), &s, keys), run(eng(keys)), "{keys}");
        }
    }

    /// Plan M3b Deviations 3 and 8: a selected dialog field reopens with its
    /// mode; a prompt's Enter confirms; a form's Enter is modal.rs's ladder.
    #[test]
    fn a_selected_dialog_field_reopens_with_its_mode() {
        let form = cx(Vim, Surface::Form);
        assert_eq!(
            feed(&mut Router::default(), &form, "i"),
            run(vec![
                Step::OpenAs(OpenAs::Insert(OpenSeat::Keep)),
                Step::Action(Action::ReopenField)
            ])
        );
        assert_eq!(
            feed(&mut Router::default(), &form, "A"),
            run(vec![
                Step::OpenAs(OpenAs::Insert(OpenSeat::End)),
                Step::Action(Action::ReopenField)
            ])
        );
        assert_eq!(
            feed(&mut Router::default(), &form, "o"),
            run(vec![Step::OpenAs(OpenAs::Normal), Step::Action(Action::ReopenField)])
        );
        assert_eq!(feed(&mut Router::default(), &form, "<CR>"), Resolved::Declined);
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Prompt), "<CR>"),
            run(vec![Step::Action(Action::ConfirmTopModal)])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Prompt), "l"),
            run(vec![key(KeyCode::Right)])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Form), "i"),
            Resolved::Declined
        );
    }

    /// Plan M3b Deviation 19: a picker with no query of its own (the
    /// dropdown) moves with vim's keys.
    #[test]
    fn a_field_less_picker_moves_with_vim_keys() {
        let s = cx(Vim, Surface::Picker);
        assert_eq!(feed(&mut Router::default(), &s, "j"), run(vec![key(KeyCode::Down)]));
        assert_eq!(feed(&mut Router::default(), &s, "k"), run(vec![key(KeyCode::Up)]));
        assert_eq!(feed(&mut Router::default(), &s, "gg"), run(vec![key(KeyCode::Home)]));
        assert_eq!(feed(&mut Router::default(), &s, "G"), run(vec![key(KeyCode::End)]));
    }

    /// Spec §5.7 and plan M5a Deviation 9: the fold keys take a count.
    #[test]
    fn the_response_folds() {
        use crate::action::FoldOp;
        let resp = cx(Vim, Surface::Response(crate::components::response::ViewMode::Pretty));
        let fold = |op, count| run(vec![Step::Action(Action::TreeFold { op, count })]);
        assert_eq!(feed(&mut Router::default(), &resp, "zc"), fold(FoldOp::Close, 1));
        assert_eq!(feed(&mut Router::default(), &resp, "3za"), fold(FoldOp::Toggle, 3));
        assert_eq!(feed(&mut Router::default(), &resp, "2zo"), fold(FoldOp::Open, 2));
        assert_eq!(feed(&mut Router::default(), &resp, "zR"), fold(FoldOp::OpenAll, 1));
        assert_eq!(feed(&mut Router::default(), &resp, "zM"), fold(FoldOp::CloseAll, 1));
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, resp.surface), "z"),
            Resolved::Declined,
            "arrows has no fold keys"
        );
    }

    /// Spec §5.8 (plan M4a Deviations 7 and 8): `gt` and `gT` step the strip;
    /// `{N}gt` selects tab N; `{N}gT` steps back N. The app lands the caret.
    #[test]
    fn tab_keys_on_a_closed_surface() {
        let s = cx(Vim, Surface::Sidebar);
        let step = |d: i64| run(vec![Step::Action(Action::GoTab { to: TabTo::By(d) })]);
        let select = |n: usize| run(vec![Step::Action(Action::GoTab { to: TabTo::Nth(n) })]);
        assert_eq!(feed(&mut Router::default(), &s, "gt"), step(1));
        assert_eq!(feed(&mut Router::default(), &s, "gT"), step(-1));
        assert_eq!(feed(&mut Router::default(), &s, "3gt"), select(3));
        assert_eq!(feed(&mut Router::default(), &s, "1gt"), select(1));
        assert_eq!(feed(&mut Router::default(), &s, "2gT"), step(-2));
        assert_eq!(feed(&mut Router::default(), &s, "14gT"), step(-14), "the strip wraps it");
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "g"),
            Resolved::Declined
        );
    }

    /// Spec §3.7's field rows (plan M4a Deviation 2): in a screen field's idle
    /// Normal the tab keys close the field first; `g` + anything else replays.
    /// The close rides in the action, which closes the field only when there
    /// is a tab to go to (amendment C1, the user's decision).
    #[test]
    fn tab_keys_in_a_screen_field() {
        let s = in_url(Mode::Normal, false);
        let go = |to| run(vec![Step::Action(Action::GoTab { to })]);
        assert_eq!(
            feed(&mut Router::default(), &s, "gt"),
            go(TabTo::By(1)),
            "no unconditional close"
        );
        assert_eq!(feed(&mut Router::default(), &s, "2gT"), go(TabTo::By(-2)));
        assert_eq!(feed(&mut Router::default(), &s, "2gt"), go(TabTo::Nth(2)));
        assert_eq!(feed(&mut Router::default(), &s, "gg"), run(eng("gg")), "gg is the engine's");
        assert_eq!(feed(&mut Router::default(), &s, "3ge"), run(eng("3ge")));
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "g"), Resolved::Consumed);
        assert_eq!(r.echo(), "g");
        let prompt = KeyCtx {
            field: FieldCaret::Live(FieldId::ModalInput(0)),
            engine: Some(EngineView { mode: Mode::Normal, pending: false }),
            ..cx(Vim, Surface::Prompt)
        };
        assert_eq!(
            feed(&mut Router::default(), &prompt, "g"),
            run(eng("g")),
            "a dialog field's g is the engine's"
        );
    }

    /// Spec §5.8 (plan M4a Deviations 3 and 4): the window keys, their ctrl
    /// spellings, a dropped count, and junk after `<C-w>`.
    #[test]
    fn window_keys_on_a_closed_surface() {
        use crate::action::PaneDir;
        let s = cx(Vim, Surface::Sidebar);
        let mv = |dir| run(vec![Step::Action(Action::PaneMove(dir))]);
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>l"), mv(PaneDir::Right));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w><C-l>"), mv(PaneDir::Right));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>W"), mv(PaneDir::Prev));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w><C-w>"), mv(PaneDir::Next));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>p"), mv(PaneDir::Last));
        assert_eq!(
            feed(&mut Router::default(), &s, "3<C-w>j"),
            mv(PaneDir::Down),
            "the count is dropped"
        );
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "<C-w>"), Resolved::Consumed);
        assert_eq!(r.echo(), "^W");
        assert_eq!(feed(&mut r, &s, "x"), Resolved::Consumed, "junk is swallowed");
        assert_eq!(
            feed(&mut r, &s, "j"),
            run(vec![key(KeyCode::Down)]),
            "and the next key is fresh"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "<C-w>"),
            Resolved::Declined
        );
    }

    /// Spec §3.7 and §5.8 (plan M4a Deviations 2 and 4): from a screen
    /// field's idle Normal the move carries the close, which the app runs
    /// only when there is a pane to move to (the user's decision,
    /// 2026-10-05); junk is swallowed; in Insert ctrl+w is the engine's.
    #[test]
    fn window_keys_in_a_screen_field() {
        use crate::action::PaneDir;
        let s = in_url(Mode::Normal, false);
        assert_eq!(
            feed(&mut Router::default(), &s, "<C-w>j"),
            run(vec![Step::Action(Action::PaneMove(PaneDir::Down))]),
            "no unconditional close: a refused move leaves the field open"
        );
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>x"), Resolved::Consumed);
        let insert = in_url(Mode::Insert, false);
        assert_eq!(
            feed(&mut Router::default(), &insert, "<C-w>"),
            run(eng("<C-w>")),
            "Insert deletes a word"
        );
    }

    /// Key list §8 (`P.ctrlw-arrows`): the arrow keys are the direction
    /// letters, on a closed surface and in a screen field's idle Normal.
    #[test]
    fn ctrl_w_arrows_are_the_direction_letters() {
        use crate::action::PaneDir;
        let mv = |dir| run(vec![Step::Action(Action::PaneMove(dir))]);
        for s in [cx(Vim, Surface::Sidebar), in_url(Mode::Normal, false)] {
            assert_eq!(feed(&mut Router::default(), &s, "<C-w><Left>"), mv(PaneDir::Left));
            assert_eq!(feed(&mut Router::default(), &s, "<C-w><Down>"), mv(PaneDir::Down));
            assert_eq!(feed(&mut Router::default(), &s, "<C-w><Up>"), mv(PaneDir::Up));
            assert_eq!(feed(&mut Router::default(), &s, "<C-w><Right>"), mv(PaneDir::Right));
        }
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "<C-w><Home>"),
            Resolved::Consumed,
            "a named key that is no target is vim's beep"
        );
        assert_eq!(
            feed(&mut Router::default(), &in_url(Mode::Normal, false), "<C-w><Home>"),
            Resolved::Consumed,
            "swallowed in a field"
        );
    }

    /// Spec S7 and E13 (task #90): Vim's window sizing keys carry their
    /// count, one with none; the app resolves them against the focused pane.
    #[test]
    fn ctrl_w_resize_keys_carry_their_count() {
        use crate::action::ResizeDir;
        let resize = |dir, count| run(vec![Step::Action(Action::PaneResize { dir, count })]);
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>>"), resize(ResizeDir::Wider, 1));
        assert_eq!(feed(&mut Router::default(), &s, "5<C-w><lt>"), resize(ResizeDir::Narrower, 5));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>+"), resize(ResizeDir::Taller, 1));
        assert_eq!(feed(&mut Router::default(), &s, "3<C-w>-"), resize(ResizeDir::Shorter, 3));
        let resp = cx(Vim, Surface::Response(crate::components::response::ViewMode::Pretty));
        assert_eq!(feed(&mut Router::default(), &resp, "2<C-w>+"), resize(ResizeDir::Taller, 2));
        assert_eq!(
            feed(&mut Router::default(), &in_url(Mode::Normal, false), "<C-w>-"),
            resize(ResizeDir::Shorter, 1),
            "from a screen field's idle Normal, as the window moves"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "<C-w>"),
            Resolved::Declined
        );
    }

    /// E13 (review round 4): `<C-w>_` sets the current window's height, to
    /// the count or, with none, as tall as it goes (Vim's 9999, saturated);
    /// `<C-w>=` evens the split and needs no count.
    #[test]
    fn ctrl_w_underscore_and_equal_route_to_the_resize() {
        use crate::action::ResizeDir;
        let resize = |dir, count| run(vec![Step::Action(Action::PaneResize { dir, count })]);
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>_"), resize(ResizeDir::Height, u16::MAX));
        assert_eq!(feed(&mut Router::default(), &s, "5<C-w>_"), resize(ResizeDir::Height, 5));
        assert_eq!(feed(&mut Router::default(), &s, "<C-w>="), resize(ResizeDir::Even, 1));
        assert_eq!(
            feed(&mut Router::default(), &s, "5<C-w>="),
            resize(ResizeDir::Even, 1),
            "a count changes nothing, so the action carries none (review round 8 N8)"
        );
        assert_eq!(
            feed(&mut Router::default(), &in_url(Mode::Normal, false), "<C-w>="),
            resize(ResizeDir::Even, 1),
            "from a screen field's idle Normal, as the window moves"
        );
    }

    /// Plan M3a Deviation 9: the prefix keys never reach the engine.
    #[test]
    fn the_prefix_keys_are_notes_or_swallowed() {
        let s = in_url(Mode::Normal, false);
        let macros =
            run(vec![Step::Action(Action::ShowToast(table::NO_MACROS.into(), ToastKind::Info))]);
        let marks =
            run(vec![Step::Action(Action::ShowToast(table::NO_MARKS.into(), ToastKind::Info))]);
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "q"), Resolved::Consumed);
        assert_eq!(r.echo(), "q");
        assert_eq!(feed(&mut r, &s, "a"), macros);
        assert_eq!(feed(&mut Router::default(), &s, "@<CR>"), macros, "any key completes it");
        assert_eq!(feed(&mut Router::default(), &s, "ma"), marks);
        assert_eq!(feed(&mut Router::default(), &s, "`x"), marks);
        assert_eq!(feed(&mut Router::default(), &s, "Zx"), Resolved::Consumed);
        assert_eq!(
            feed(&mut Router::default(), &s, ":"),
            run(vec![Step::Action(Action::OpenExPalette)])
        );
        let mut r = Router::default();
        feed(&mut r, &s, "q");
        assert_eq!(feed(&mut r, &s, "<Esc>"), Resolved::Consumed, "Esc cancels the pair silently");
        assert_eq!(feed(&mut r, &s, "x"), run(eng("x")), "and the next key is fresh");
    }

    /// A field the sync has not opened yet still types.
    #[test]
    fn a_field_without_an_engine_session_types() {
        let s = KeyCtx { field: FieldCaret::Live(FieldId::Palette), ..cx(Vim, Surface::Picker) };
        assert_eq!(feed(&mut Router::default(), &s, "j"), Resolved::Declined);
        let unsynced = KeyCtx { engine: None, ..in_url(Mode::Normal, false) };
        assert_eq!(feed(&mut Router::default(), &unsynced, "j"), Resolved::Declined);
        let arrows = KeyCtx { profile: Arrows, ..in_url(Mode::Normal, false) };
        assert_eq!(
            feed(&mut Router::default(), &arrows, "j"),
            Resolved::Declined,
            "arrows has no engine"
        );
    }

    /// Spec §5.6 and plan M3a Deviation 12: the opening keys say how.
    #[test]
    fn opening_keys_carry_their_open_mode() {
        let enter = key(KeyCode::Enter);
        let open = |seat| run(vec![Step::OpenAs(OpenAs::Insert(seat)), enter.clone()]);
        let url = cx(Vim, Surface::UrlLine);
        assert_eq!(feed(&mut Router::default(), &url, "i"), open(OpenSeat::Keep));
        assert_eq!(feed(&mut Router::default(), &url, "a"), open(OpenSeat::After));
        assert_eq!(feed(&mut Router::default(), &url, "I"), open(OpenSeat::FirstNonBlank));
        assert_eq!(feed(&mut Router::default(), &url, "A"), open(OpenSeat::End));
        let table = cx(Vim, Surface::Table(crate::components::editor::EditorTab::Headers));
        assert_eq!(feed(&mut Router::default(), &table, "i"), open(OpenSeat::Keep));
        assert_eq!(
            feed(&mut Router::default(), &table, "a"),
            run(vec![sel(crate::action::Verb::Add)]),
            "an add verb's Insert is the app's `OnSelection` arm's (ex_tests' add-verb test)"
        );
        assert_eq!(
            feed(&mut Router::default(), &table, "o"),
            run(vec![enter]),
            "o opens in Normal"
        );
    }

    /// Spec §8.3 for the field classes: whatever follows a field prefix,
    /// the machine ends idle, and a following `x` is fresh.
    #[test]
    fn unknown_after_a_field_prefix_never_leaks() {
        let s = in_url(Mode::Normal, false);
        for prefix in ["q", "@", "m", "'", "`", "Z", "g", "<C-w>"] {
            for c in (b'!'..=b'~').map(char::from) {
                let mut r = Router::default();
                feed(&mut r, &s, prefix);
                r.resolve(Tok::Char(c).event(), &s);
                assert_eq!(r.echo(), "", "{prefix}{c}");
                assert_eq!(
                    r.resolve(Tok::Char('x').event(), &s),
                    run(eng("x")),
                    "{prefix}{c}: x is fresh"
                );
            }
        }
    }

    /// Feeds `keys`, returning what the last key resolved to.
    fn feed(r: &mut Router, cx: &KeyCtx, keys: &str) -> Resolved {
        let mut last = Resolved::Declined;
        for ev in notation::events(keys) {
            last = r.resolve(ev, cx);
        }
        last
    }

    fn key(code: KeyCode) -> Step {
        Step::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl_key(code: KeyCode) -> Step {
        Step::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
    }

    fn run(steps: Vec<Step>) -> Resolved {
        Resolved::Run(steps)
    }

    #[test]
    fn j_moves_in_vim_and_is_declined_in_arrows() {
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "j"),
            run(vec![key(KeyCode::Down)])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "j"),
            Resolved::Declined
        );
        let mut r = Router::default();
        assert_eq!(
            feed(&mut r, &cx(Arrows, Surface::Sidebar), "3"),
            Resolved::Declined,
            "no counts in arrows"
        );
        assert_eq!(r.echo(), "");
    }

    #[test]
    fn a_count_repeats_a_motion_capped_by_the_surface() {
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "3j"),
            run(vec![key(KeyCode::Down); 3])
        );
        let small = KeyCtx { cap: 2, ..cx(Vim, Surface::Sidebar) };
        assert_eq!(feed(&mut Router::default(), &small, "9j"), run(vec![key(KeyCode::Down); 2]));
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "3o"),
            run(vec![key(KeyCode::Enter)]),
            "o ignores it"
        );
    }

    #[test]
    fn gg_and_g_with_and_without_counts() {
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "gg"), run(vec![key(KeyCode::Home)]));
        assert_eq!(feed(&mut Router::default(), &s, "G"), run(vec![key(KeyCode::End)]));
        let row3 = run(vec![key(KeyCode::Home), key(KeyCode::Down), key(KeyCode::Down)]);
        assert_eq!(feed(&mut Router::default(), &s, "3G"), row3);
        assert_eq!(feed(&mut Router::default(), &s, "3gg"), row3);
        let resp = cx(Vim, Surface::Response(crate::components::response::ViewMode::Raw));
        use crate::action::ResponseMotion as M;
        let mv = |m| run(vec![Step::Action(Action::ResponseMove(m))]);
        assert_eq!(
            feed(&mut Router::default(), &resp, "G"),
            mv(M::Goto { line: None, last: true })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "10G"),
            mv(M::Goto { line: Some(10), last: true })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "gg"),
            mv(M::Goto { line: None, last: false })
        );
        assert_eq!(feed(&mut Router::default(), &resp, "5j"), mv(M::Lines(5)));
        assert_eq!(feed(&mut Router::default(), &resp, "<Up>"), mv(M::Lines(-1)));
        assert_eq!(
            feed(&mut Router::default(), &resp, "3<C-d>"),
            mv(M::HalfPage { down: true, count: Some(3) })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "2<C-f>"),
            mv(M::Page { down: true, count: 2 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "12zt"),
            mv(M::Place { at: crate::action::Place::Top, line: Some(12) })
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, resp.surface), "j"),
            Resolved::Declined,
            "arrows' j is a letter"
        );
    }

    #[test]
    fn a_prefix_waits_and_echoes() {
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &cx(Vim, Surface::Sidebar), "2g"), Resolved::Consumed);
        assert_eq!(r.echo(), "2g");
    }

    #[test]
    fn esc_clears_whatever_is_pending() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::Sidebar);
        feed(&mut r, &s, "3g");
        assert_eq!(feed(&mut r, &s, "<Esc>"), Resolved::Consumed);
        assert_eq!(r.echo(), "");
        assert_eq!(feed(&mut r, &s, "<Esc>"), Resolved::Declined, "idle Esc is today's Esc");
    }

    /// Review Focus 5.
    #[test]
    fn a_chord_after_a_prefix_is_declined() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::Sidebar);
        feed(&mut r, &s, "g");
        assert_eq!(
            r.resolve(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), &s),
            Resolved::Declined
        );
        assert_eq!(r.echo(), "");
        feed(&mut r, &s, "3");
        assert_eq!(
            r.resolve(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT), &s),
            Resolved::Declined
        );
        assert_eq!(r.echo(), "", "an alt chord drops a count too");
    }

    #[test]
    fn an_unknown_key_after_a_prefix_is_swallowed() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut r, &s, "gx"), Resolved::Consumed);
        assert_eq!(feed(&mut r, &s, "j"), run(vec![key(KeyCode::Down)]), "the next key is fresh");
    }

    #[test]
    fn a_bare_char_with_no_row_is_swallowed_on_navigation_surfaces_only() {
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "x"),
            Resolved::Consumed
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Dialog), "x"),
            Resolved::Declined,
            "its letters answer"
        );
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Other), "x"), Resolved::Declined);
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "<Space>"),
            Resolved::Declined,
            "Space is the surface's"
        );
    }

    #[test]
    fn a_count_then_an_arrow_repeats_it_and_any_other_key_drops_it() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut r, &s, "3<Down>"), run(vec![key(KeyCode::Down); 3]));
        assert_eq!(feed(&mut r, &s, "3<CR>"), Resolved::Declined);
        assert_eq!(r.echo(), "", "a stray count never multiplies a later key");
    }

    #[test]
    fn a_surface_change_clears_the_pending_keys() {
        let mut r = Router::default();
        feed(&mut r, &cx(Vim, Surface::Sidebar), "3");
        assert_eq!(
            feed(
                &mut r,
                &cx(Vim, Surface::Table(crate::components::editor::EditorTab::Params)),
                "j"
            ),
            run(vec![key(KeyCode::Down)])
        );
    }

    #[test]
    fn a_live_field_takes_every_key() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::UrlLine);
        feed(&mut r, &s, "3");
        let live = KeyCtx { field: FieldCaret::Live(FieldId::Url), ..s };
        assert_eq!(feed(&mut r, &live, "j"), Resolved::Declined);
        assert_eq!(r.echo(), "");
    }

    #[test]
    fn a_keys_toml_bare_char_wins_while_idle() {
        let mut r = Router::default();
        let bound = KeyCtx { user_bare: Some(Action::Quit), ..cx(Vim, Surface::Sidebar) };
        assert_eq!(feed(&mut r, &bound, "x"), run(vec![Step::Action(Action::Quit)]));
        // `g` is typed with nothing bound to it, so a prefix waits; the
        // binding on `x` must not fire while it does.
        assert_eq!(feed(&mut r, &cx(Vim, Surface::Sidebar), "g"), Resolved::Consumed);
        assert_eq!(feed(&mut r, &bound, "x"), Resolved::Consumed, "not while a prefix waits");
    }

    fn sel(v: crate::action::Verb) -> Step {
        Step::Action(Action::OnSelection(v))
    }

    /// Spec §5.1: the list verbs in vim: `dd` passes its count, `yy`
    /// refuses one, `p` passes its count (capped by the surface), shared letters
    /// once.
    #[test]
    fn vim_list_verbs_resolve_to_on_selection() {
        use crate::action::Verb;
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(
            feed(&mut Router::default(), &s, "dd"),
            run(vec![sel(Verb::Delete { count: 1 })])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "3dd"),
            run(vec![sel(Verb::Delete { count: 3 })])
        );
        assert_eq!(feed(&mut Router::default(), &s, "yy"), run(vec![sel(Verb::Yank)]));
        assert_eq!(
            feed(&mut Router::default(), &s, "3yy"),
            run(vec![Step::Action(Action::ShowToast(
                "Counted yank is not supported".into(),
                ToastKind::Info
            ))])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "1yy"),
            run(vec![sel(Verb::Yank)]),
            "1yy is yy, as in vim"
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "p"),
            run(vec![sel(Verb::Put { above: false, count: 1 })])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "2p"),
            run(vec![sel(Verb::Put { above: false, count: 2 })])
        );
        let capped = KeyCtx { cap: 4, ..s.clone() };
        assert_eq!(
            feed(&mut Router::default(), &capped, "9P"),
            run(vec![sel(Verb::Put { above: true, count: 4 })]),
            "a put's count is capped by the surface"
        );
        assert_eq!(feed(&mut Router::default(), &s, "a"), run(vec![sel(Verb::Add)]));
        assert_eq!(
            feed(&mut Router::default(), &s, "3r"),
            run(vec![sel(Verb::Rename)]),
            "a count is ignored"
        );
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "d"), Resolved::Consumed);
        assert_eq!(r.echo(), "d");
    }

    /// Plan M2 Deviation 14: a counted `dd` is refused off the sidebar and
    /// the tables; one `dd` (or `1dd`) still deletes.
    #[test]
    fn a_counted_dd_is_refused_on_the_manage_and_variables_lists() {
        use crate::action::Verb;
        use crate::components::manage::ManageTab;
        let note = Step::Action(Action::ShowToast(
            "Counted delete works on requests and table rows".into(),
            ToastKind::Info,
        ));
        for surface in
            [Surface::ManageList(ManageTab::Environments), Surface::VarList, Surface::VarGrid]
        {
            let s = cx(Vim, surface);
            assert_eq!(
                feed(&mut Router::default(), &s, "2dd"),
                run(vec![note.clone()]),
                "{surface:?}"
            );
            assert_eq!(
                feed(&mut Router::default(), &s, "dd"),
                run(vec![sel(Verb::Delete { count: 1 })]),
                "{surface:?}"
            );
            assert_eq!(
                feed(&mut Router::default(), &s, "1dd"),
                run(vec![sel(Verb::Delete { count: 1 })]),
                "{surface:?}"
            );
        }
    }

    /// Spec §4.5: the arrows profile's verbs are one letter each, and its
    /// old letters are gone.
    #[test]
    fn arrows_list_verbs_are_one_letter() {
        use crate::action::Verb;
        use crate::components::manage::ManageTab;
        let s = cx(Arrows, Surface::Sidebar);
        assert_eq!(
            feed(&mut Router::default(), &s, "d"),
            run(vec![sel(Verb::Delete { count: 1 })])
        );
        assert_eq!(feed(&mut Router::default(), &s, "y"), run(vec![sel(Verb::Yank)]));
        assert_eq!(
            feed(&mut Router::default(), &s, "P"),
            run(vec![sel(Verb::Put { above: true, count: 1 })])
        );
        assert_eq!(feed(&mut Router::default(), &s, "a"), run(vec![sel(Verb::Add)]));
        assert_eq!(feed(&mut Router::default(), &s, "n"), Resolved::Declined, "n is no longer new");
        let env = cx(Arrows, Surface::ManageDetail(ManageTab::Environments));
        assert_eq!(feed(&mut Router::default(), &env, "t"), run(vec![sel(Verb::Tls)]));
        let form = cx(Arrows, Surface::VarForm);
        assert_eq!(feed(&mut Router::default(), &form, "r"), run(vec![sel(Verb::Reveal)]));
        assert_eq!(feed(&mut Router::default(), &form, "p"), run(vec![sel(Verb::Promote)]));
        let grid = cx(Arrows, Surface::VarGrid);
        assert_eq!(feed(&mut Router::default(), &grid, "y"), run(vec![sel(Verb::Yank)]));
        assert_eq!(
            feed(&mut Router::default(), &grid, "p"),
            run(vec![sel(Verb::Put { above: false, count: 1 })])
        );
        assert_eq!(
            feed(&mut Router::default(), &grid, "P"),
            run(vec![sel(Verb::Put { above: true, count: 1 })])
        );
        // #145: the old c/v pair is gone.
        assert_eq!(feed(&mut Router::default(), &grid, "c"), Resolved::Declined);
        assert_eq!(feed(&mut Router::default(), &grid, "v"), Resolved::Declined);
        assert_eq!(
            feed(&mut Router::default(), &grid, "o"),
            run(vec![Step::Action(Action::StartNewOptionEdit)])
        );
    }

    /// Plan M2 Deviation 1: vim takes only the shared letters on the
    /// Variables form; its other verbs wait for M4's ex rows.
    #[test]
    fn vim_swallows_the_arrows_only_verb_letters() {
        for (surface, keys) in
            [(Surface::VarForm, "r"), (Surface::VarForm, "s"), (Surface::VarGrid, "e")]
        {
            assert_eq!(
                feed(&mut Router::default(), &cx(Vim, surface), keys),
                Resolved::Consumed,
                "{surface:?} {keys}"
            );
        }
    }

    /// Plan M2 Deviation 18 (amendment A5-1): a `keys.toml` letter wins
    /// over an arrows verb row, as it does over a vim one.
    #[test]
    fn an_arrows_keys_toml_letter_wins_over_a_verb_row() {
        let bound = KeyCtx { user_bare: Some(Action::Quit), ..cx(Arrows, Surface::Sidebar) };
        assert_eq!(
            feed(&mut Router::default(), &bound, "y"),
            run(vec![Step::Action(Action::Quit)])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "y"),
            run(vec![sel(crate::action::Verb::Yank)])
        );
    }

    /// Deviation 22: a digit is a count only where some row takes one.
    #[test]
    fn a_digit_is_a_count_only_on_surfaces_that_take_one() {
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &cx(Vim, Surface::Dialog), "3"), Resolved::Declined);
        assert_eq!(r.echo(), "", "pending stays idle");
        assert_eq!(
            feed(&mut r, &cx(Vim, Surface::Sidebar), "3j"),
            run(vec![key(KeyCode::Down); 3])
        );
        for surface in [Surface::Prompt, Surface::Form, Surface::Other] {
            let mut r = Router::default();
            assert_eq!(feed(&mut r, &cx(Vim, surface), "3"), Resolved::Declined, "{surface:?}");
            assert_eq!(r.echo(), "", "{surface:?}: pending stays idle");
        }
    }

    #[test]
    fn a_lone_count_is_cleared_by_esc_and_dropped_by_a_swallowed_key() {
        let s = cx(Vim, Surface::Sidebar);
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "3<Esc>"), Resolved::Consumed);
        assert_eq!(r.echo(), "");
        assert_eq!(feed(&mut r, &s, "3x"), Resolved::Consumed);
        assert_eq!(r.echo(), "", "the count is dropped and x is swallowed");
    }

    #[test]
    fn a_leading_zero_is_a_key() {
        let resp = cx(Vim, Surface::Response(crate::components::response::ViewMode::Pretty));
        let start = crate::action::ResponseMotion::Col(crate::action::ColMove::Start);
        assert_eq!(
            feed(&mut Router::default(), &resp, "0"),
            run(vec![Step::Action(Action::ResponseMove(start))])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "0"),
            Resolved::Consumed
        );
    }

    #[test]
    fn undo_redo_and_the_palette_are_rows() {
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "u"), run(vec![Step::Action(Action::Undo)]));
        assert_eq!(
            feed(&mut Router::default(), &s, "3u"),
            run(vec![Step::Action(Action::Undo); 3])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "<C-r>"),
            run(vec![Step::Action(Action::Redo)])
        );
        assert_eq!(
            feed(&mut Router::default(), &s, "3:"),
            run(vec![Step::Action(Action::OpenExPalette)])
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Picker), "u"),
            Resolved::Declined,
            "a picker types it"
        );
    }

    /// Spec §4.5 (plan M4a Deviation 11): the response's letters are rows in
    /// both profiles. Vim's `n` `N` `*` `#` take a count and `?` searches
    /// backward (plan M5a Task 5); arrows' `n` `N` step. The lists' `/` is a
    /// note.
    #[test]
    fn the_response_letters_are_rows() {
        use crate::components::response::ViewMode;
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "/"),
            run(vec![Step::Action(Action::ShowToast(
                "No search in this list yet".into(),
                ToastKind::Info
            ))])
        );
        let search = run(vec![Step::Action(Action::OpenResponseSearch { backward: false })]);
        let next = run(vec![Step::Action(Action::StepResponseSearch(1))]);
        let prev = run(vec![Step::Action(Action::StepResponseSearch(-1))]);
        use crate::action::ResponseFind;
        for profile in [Vim, Arrows] {
            let resp = cx(profile, Surface::Response(ViewMode::Pretty));
            assert_eq!(feed(&mut Router::default(), &resp, "/"), search, "{profile:?}");
        }
        let arrows = cx(Arrows, Surface::Response(ViewMode::Pretty));
        assert_eq!(feed(&mut Router::default(), &arrows, "n"), next);
        assert_eq!(feed(&mut Router::default(), &arrows, "N"), prev);
        let resp = cx(Vim, Surface::Response(ViewMode::Headers));
        let find = |f| run(vec![Step::Action(Action::ResponseFind(f))]);
        assert_eq!(
            feed(&mut Router::default(), &resp, "n"),
            find(ResponseFind::Next { reverse: false, count: 1 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "3N"),
            find(ResponseFind::Next { reverse: true, count: 3 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "2*"),
            find(ResponseFind::Word { backward: false, count: 2 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "#"),
            find(ResponseFind::Word { backward: true, count: 1 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "?"),
            run(vec![Step::Action(Action::OpenResponseSearch { backward: true })])
        );
        assert_eq!(feed(&mut Router::default(), &resp, "c"), Resolved::Consumed, "vim has no c");
    }

    /// Spec 2026-10-26 H4 and H8: the response's header copy is arrows'
    /// `y` (the value) and `Y` (the whole header), and vim's `"+yy` and
    /// `"+Y` (the line). Arrows' old `c` is gone.
    #[test]
    fn the_response_header_copy_keys() {
        use crate::components::response::ViewMode;
        let copy = |whole| run(vec![Step::Action(Action::CopyRow { whole })]);
        let arrows = cx(Arrows, Surface::Response(ViewMode::Headers));
        assert_eq!(feed(&mut Router::default(), &arrows, "y"), copy(false));
        assert_eq!(feed(&mut Router::default(), &arrows, "Y"), copy(true));
        assert_eq!(feed(&mut Router::default(), &arrows, "c"), Resolved::Declined);
        let vim = cx(Vim, Surface::Response(ViewMode::Headers));
        assert_eq!(feed(&mut Router::default(), &vim, "\"+yy"), copy(true));
        assert_eq!(feed(&mut Router::default(), &vim, "\"+Y"), copy(true));
    }

    /// Spec 2026-10-26 H2 and H8: a header row's whole-header copy is
    /// arrows' `Y` and vim's `"+yy` / `"+Y`; `"+` waits for them, and any
    /// other key after it is still the registers note.
    #[test]
    fn the_table_header_copy_keys() {
        use crate::action::Verb;
        use crate::components::editor::EditorTab;
        let note = |t: &str| run(vec![Step::Action(Action::ShowToast(t.into(), ToastKind::Info))]);
        let whole = || run(vec![Step::Action(Action::CopyRow { whole: true })]);
        let arrows = cx(Arrows, Surface::Table(EditorTab::Headers));
        assert_eq!(feed(&mut Router::default(), &arrows, "Y"), whole());
        assert_eq!(feed(&mut Router::default(), &arrows, "y"), run(vec![sel(Verb::Yank)]));
        let vim = cx(Vim, Surface::Table(EditorTab::Headers));
        assert_eq!(feed(&mut Router::default(), &vim, "\"+yy"), whole());
        assert_eq!(feed(&mut Router::default(), &vim, "\"+Y"), whole());
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &vim, "\"+"), Resolved::Consumed, "\"+ waits");
        assert_eq!(feed(&mut r, &vim, "y"), Resolved::Consumed, "\"+y waits");
        assert_eq!(feed(&mut Router::default(), &vim, "\"+j"), note(table::NO_REGISTERS));
        assert_eq!(feed(&mut Router::default(), &vim, "\"+yj"), note(table::NO_REGISTERS));
        assert_eq!(feed(&mut Router::default(), &vim, "\"a"), note(table::NO_REGISTERS));
        assert_eq!(feed(&mut Router::default(), &vim, "yy"), run(vec![sel(Verb::Yank)]));
    }

    /// Plan M5a Deviation 11: the response's V surface.
    #[test]
    fn v_line_rows() {
        use crate::action::{LinesOp, ResponseMotion as M};
        use crate::components::response::ViewMode;
        let resp = cx(Vim, Surface::Response(ViewMode::Raw));
        let vis = cx(Vim, Surface::ResponseLines(ViewMode::Raw));
        let act = |a| run(vec![Step::Action(a)]);
        assert_eq!(
            feed(&mut Router::default(), &resp, "V"),
            act(Action::ResponseLines(LinesOp::Start))
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "3yy"),
            act(Action::ResponseLines(LinesOp::Yank { count: Some(3) }))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "3j"),
            act(Action::ResponseExtend(M::Lines(3)))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "G"),
            act(Action::ResponseExtend(M::Goto { line: None, last: true }))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "y"),
            act(Action::ResponseLines(LinesOp::Yank { count: None }))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "o"),
            act(Action::ResponseLines(LinesOp::Swap))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "V"),
            act(Action::ResponseLines(LinesOp::End))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "<Esc>"),
            act(Action::ResponseLines(LinesOp::End))
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "v"),
            run(vec![Step::Action(Action::ShowToast(table::ONLY_LINES.into(), ToastKind::Info))])
        );
        assert_eq!(feed(&mut Router::default(), &vis, "u"), Resolved::Consumed, "swallowed in V");
        assert_eq!(
            feed(&mut Router::default(), &vis, ":"),
            act(Action::OpenExPalette),
            "Vim's : in Visual"
        );
        let close_all = Action::TreeFold { op: crate::action::FoldOp::CloseAll, count: 1 };
        assert_eq!(feed(&mut Router::default(), &vis, "zM"), act(close_all), "Vim's zM in Visual");
        assert_eq!(
            feed(&mut Router::default(), &vis, "<C-w>k"),
            act(Action::PaneMove(crate::action::PaneDir::Up))
        );
        let search = act(Action::OpenResponseSearch { backward: false });
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, vis.surface), "/"),
            search,
            "arrows' / with a selection up"
        );
    }

    /// Amendment A6.3 (probe V14 in the Task 6 report): in V, PageDown and
    /// PageUp extend as ctrl+f and ctrl+b do, counted. On the plain
    /// response they move as ctrl+f and ctrl+b do, counted too; arrows'
    /// stay the component's named keys.
    #[test]
    fn v_line_page_keys_extend() {
        use crate::action::ResponseMotion as M;
        use crate::components::response::ViewMode;
        let vis = cx(Vim, Surface::ResponseLines(ViewMode::Pretty));
        let ext = |m| run(vec![Step::Action(Action::ResponseExtend(m))]);
        assert_eq!(
            feed(&mut Router::default(), &vis, "<PageDown>"),
            ext(M::Page { down: true, count: 1 })
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "3<PageUp>"),
            ext(M::Page { down: false, count: 3 })
        );
        assert_eq!(
            feed(&mut Router::default(), &vis, "<C-f>"),
            ext(M::Page { down: true, count: 1 })
        );
        let resp = cx(Vim, Surface::Response(ViewMode::Pretty));
        let mv = |m| run(vec![Step::Action(Action::ResponseMove(m))]);
        assert_eq!(
            feed(&mut Router::default(), &resp, "3<PageDown>"),
            mv(M::Page { down: true, count: 3 })
        );
        assert_eq!(
            feed(&mut Router::default(), &resp, "<PageUp>"),
            mv(M::Page { down: false, count: 1 })
        );
        let arrows = cx(Arrows, Surface::Response(ViewMode::Pretty));
        assert_eq!(feed(&mut Router::default(), &arrows, "<PageDown>"), Resolved::Declined);
    }

    /// Vim's Enter (`+`) and Space (`l` that wraps) on the response, in V
    /// and out (the user's decision, 2026-10-10: Vim's, in vim); arrows
    /// keeps main's Space and Enter, the component's.
    #[test]
    fn vims_enter_and_space_move_on_the_response() {
        use crate::action::{ColMove, ResponseMotion as M};
        use crate::components::response::ViewMode;
        let vis = cx(Vim, Surface::ResponseLines(ViewMode::Pretty));
        let ext = |m| run(vec![Step::Action(Action::ResponseExtend(m))]);
        assert_eq!(feed(&mut Router::default(), &vis, "<CR>"), ext(M::NextLine(1)));
        assert_eq!(feed(&mut Router::default(), &vis, "3<CR>"), ext(M::NextLine(3)));
        assert_eq!(feed(&mut Router::default(), &vis, "<Space>"), ext(M::Col(ColMove::Space(1))));
        let resp = cx(Vim, Surface::Response(ViewMode::Pretty));
        let mv = |m| run(vec![Step::Action(Action::ResponseMove(m))]);
        assert_eq!(feed(&mut Router::default(), &resp, "2<Space>"), mv(M::Col(ColMove::Space(2))));
        assert_eq!(feed(&mut Router::default(), &resp, "<CR>"), mv(M::NextLine(1)));
        for surface in
            [Surface::ResponseLines(ViewMode::Pretty), Surface::Response(ViewMode::Pretty)]
        {
            let arrows = cx(Arrows, surface);
            assert_eq!(feed(&mut Router::default(), &arrows, "<CR>"), Resolved::Declined);
            assert_eq!(feed(&mut Router::default(), &arrows, "<Space>"), Resolved::Declined);
        }
    }

    /// Review Focus 4.
    #[test]
    fn counts_saturate() {
        let keys = "9".repeat(30) + "j";
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), &keys),
            run(vec![key(KeyCode::Down); MAX_COUNT])
        );
    }

    /// Spec §8.3: whatever follows a prefix, the machine ends Idle unless
    /// the two together are still a prefix, and a following `j` is fresh.
    #[test]
    fn unknown_after_prefix_never_leaks() {
        for profile in [Arrows, Vim] {
            for surface in Surface::SAMPLES {
                let mut prefixes: Vec<Vec<Tok>> = Vec::new();
                for b in table::TABLE.iter().filter(|b| b.profile.has(profile) && b.on.has(surface))
                {
                    let keys = notation::parse(b.keys);
                    for n in 1..keys.len() {
                        prefixes.push(keys[..n].to_vec());
                    }
                }
                for prefix in prefixes {
                    for c in (b'!'..=b'~').map(char::from) {
                        let s = cx(profile, surface);
                        let mut r = Router::default();
                        for t in &prefix {
                            assert_eq!(r.resolve(t.event(), &s), Resolved::Consumed);
                        }
                        r.resolve(Tok::Char(c).event(), &s);
                        let mut both = prefix.clone();
                        both.push(Tok::Char(c));
                        let still =
                            matches!(table::lookup(&both, profile, surface), table::Match::Prefix);
                        assert_eq!(
                            r.echo().is_empty(),
                            !still,
                            "{profile:?} {surface:?} {prefix:?} then {c:?}"
                        );
                        if !still {
                            assert_eq!(
                                r.resolve(Tok::Char('j').event(), &s),
                                Router::default().resolve(Tok::Char('j').event(), &s),
                                "{profile:?} {surface:?} {prefix:?} {c:?}: j is fresh"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Spec §5.11, key list G.ZZ: `ZZ` is `:x`, `ZQ` is `:q!` (on the main
    /// screen they quit, on Manage they close it: `Action::CloseWindow`);
    /// in a screen field's idle Normal `ZZ` closes the field first
    /// (Deviation 2); a dialog's field keeps swallowing `Z<any>`.
    #[test]
    fn zz_and_zq_quit() {
        let zz = || Step::Action(Action::CloseWindow { write: true, force: false });
        let zq = || Step::Action(Action::CloseWindow { write: false, force: true });
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "ZZ"), run(vec![zz()]));
        assert_eq!(feed(&mut Router::default(), &s, "ZQ"), run(vec![zq()]));
        assert_eq!(feed(&mut Router::default(), &s, "3ZZ"), run(vec![zz()]), "a count is dropped");
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "Zx"), Resolved::Consumed, "vim's beep");
        assert_eq!(feed(&mut r, &s, "j"), run(vec![key(KeyCode::Down)]));
        let url = in_url(Mode::Normal, false);
        assert_eq!(
            feed(&mut Router::default(), &url, "ZZ"),
            run(vec![Step::Action(Action::CloseField), zz()])
        );
        assert_eq!(feed(&mut Router::default(), &url, "ZQ"), run(vec![zq()]));
        assert_eq!(feed(&mut Router::default(), &url, "Zx"), Resolved::Consumed);
        let prompt = KeyCtx {
            field: FieldCaret::Live(FieldId::ModalInput(0)),
            engine: Some(EngineView { mode: Mode::Normal, pending: false }),
            ..cx(Vim, Surface::Prompt)
        };
        assert_eq!(
            feed(&mut Router::default(), &prompt, "ZZ"),
            Resolved::Consumed,
            "a dialog field swallows Z<any>"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Dialog), "Z"),
            Resolved::Declined,
            "no row on a dialog"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "Z"),
            Resolved::Declined
        );
    }

    /// Spec §5.11, key list G.qa/G.marks/G.regs: the notes on a list, and
    /// the recording flag (Deviations 4 and 5).
    #[test]
    fn the_notes_and_the_recording_flag_on_a_list() {
        let s = cx(Vim, Surface::Sidebar);
        let note = |t: &str| run(vec![Step::Action(Action::ShowToast(t.into(), ToastKind::Info))]);
        assert_eq!(feed(&mut Router::default(), &s, "@a"), note(table::NO_MACROS));
        assert_eq!(feed(&mut Router::default(), &s, "'a"), note(table::NO_MARKS));
        assert_eq!(feed(&mut Router::default(), &s, "`a"), note(table::NO_MARKS));
        assert_eq!(feed(&mut Router::default(), &s, "\"a"), note(table::NO_REGISTERS));
        assert_eq!(
            feed(&mut Router::default(), &s, "m"),
            run(vec![sel(crate::action::Verb::Move)]),
            "m stays move on a list"
        );
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "qa"), note(table::NO_MACROS));
        assert_eq!(r.recording(), Some('a'));
        assert_eq!(feed(&mut r, &s, "x"), Resolved::Consumed, "x is vim's beep on a list");
        assert_eq!(feed(&mut r, &s, "q"), Resolved::Consumed, "the closing q is swallowed");
        assert_eq!(r.recording(), None);
        assert_eq!(feed(&mut r, &s, "q"), Resolved::Consumed, "and the next q is a fresh prefix");
        assert_eq!(r.echo(), "q");
        assert_eq!(feed(&mut r, &s, "<CR>"), note(table::NO_MACROS), "q<CR>: the note only");
        assert_eq!(r.recording(), None, "Enter is no register");
        feed(&mut r, &s, "q\"");
        assert_eq!(r.recording(), Some('"'));
        // A field close drops the pending keys and keeps the flag (the
        // final review's ruling, I-4); a mouse press or a profile switch
        // drops both.
        assert_eq!(feed(&mut r, &s, "g"), Resolved::Consumed);
        assert_eq!(r.echo(), "g");
        r.clear_pending();
        assert_eq!(r.echo(), "", "the pending keys go");
        assert_eq!(r.recording(), Some('"'), "the flag stays");
        r.clear();
        assert_eq!(r.recording(), None, "a clear drops the flag");
    }

    /// The recording flag in a field's idle Normal: the closing `q` is
    /// swallowed there too, and in Insert `q` is the engine's.
    #[test]
    fn the_recording_flag_in_a_field() {
        let s = in_url(Mode::Normal, false);
        let mut r = Router::default();
        feed(&mut r, &s, "qa");
        assert_eq!(r.recording(), Some('a'));
        assert_eq!(feed(&mut r, &s, "q"), Resolved::Consumed);
        assert_eq!(r.recording(), None);
        assert_eq!(feed(&mut r, &s, "x"), run(eng("x")), "x is fresh");
        let mut r = Router::default();
        feed(&mut r, &s, "qa");
        let insert = in_url(Mode::Insert, false);
        assert_eq!(feed(&mut r, &insert, "q"), run(eng("q")), "Insert types q");
        assert_eq!(r.recording(), Some('a'), "and the flag stays");
    }

    /// Spec §5.12 (plan M5b Deviation 7): vim's `dd`, `p` and `P` note the
    /// change they ran, count and all; a yank, a move, a fold or arrows'
    /// `d` do not.
    #[test]
    fn the_list_changes_are_noted_for_dot() {
        let mut r = Router::default();
        let s = cx(Vim, Surface::Sidebar);
        feed(&mut r, &s, "3dd");
        assert_eq!(r.take_change(), Some(Change { keys: "dd", count: Some(3) }));
        assert_eq!(r.take_change(), None, "once");
        feed(&mut r, &s, "P");
        assert_eq!(r.take_change(), Some(Change { keys: "P", count: None }));
        for keys in ["yy", "j", "zo"] {
            feed(&mut r, &s, keys);
            assert_eq!(r.take_change(), None, "{keys}");
        }
        let mut r = Router::default();
        feed(&mut r, &cx(Arrows, Surface::Sidebar), "d");
        assert_eq!(r.take_change(), None, "arrows has no `.`");
    }

    /// `.` replays the change's keys where they run, by the surface's own
    /// rules (plan M5b Deviation 2), and reads its own count.
    #[test]
    fn dot_replays_the_keys_where_they_run() {
        use crate::action::Verb;
        use crate::components::manage::ManageTab;
        let dd = Change { keys: "dd", count: Some(2) };
        assert_eq!(
            replay(dd, Surface::Sidebar, MAX_COUNT),
            Some(vec![Step::Action(Action::OnSelection(Verb::Delete { count: 2 }))])
        );
        assert_eq!(
            replay(dd, Surface::ManageList(ManageTab::Environments), MAX_COUNT),
            Some(vec![note_step(table::NO_COUNTED_DELETE)]),
            "the list's own refusal, as a typed `2dd`"
        );
        assert_eq!(
            replay(dd, Surface::ManageDetail(ManageTab::Environments), MAX_COUNT),
            None,
            "no `dd` there"
        );
        assert_eq!(
            replay(dd, Surface::Response(crate::components::response::ViewMode::Pretty), MAX_COUNT),
            None
        );
        assert_eq!(
            replay(Change { keys: "p", count: Some(9) }, Surface::Sidebar, 4),
            Some(vec![Step::Action(Action::OnSelection(Verb::Put { above: false, count: 4 }))]),
            "capped as a typed put"
        );
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "3."),
            run(vec![Step::Action(Action::RepeatChange { count: Some(3) })])
        );
        assert_eq!(
            feed(
                &mut Router::default(),
                &cx(Vim, Surface::Table(crate::components::editor::EditorTab::Headers)),
                "."
            ),
            run(vec![Step::Action(Action::RepeatChange { count: None })])
        );
    }
}
