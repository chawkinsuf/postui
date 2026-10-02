//! The key router (piece 4 spec §3): where a key lands, what a key means
//! there, and what is half-typed. `App` builds the context once per key
//! (`app/router.rs`); everything in this module is pure.

pub mod ctx;
#[cfg(test)]
pub mod keylist;
pub mod notation;
pub mod pending;
pub mod spell;
pub mod table;

pub use ctx::{FieldCaret, FieldId, KeyCtx, SpellCtx, Surface};

use crate::action::Action;
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

/// The one pending machine (spec §3.4).
#[derive(Debug, Default)]
pub struct Router {
    pending: Pending,
    /// The surface the pending keys were typed on.
    surface: Option<Surface>,
}

impl Router {
    /// Drops whatever is pending: a profile switch, a mouse press.
    pub fn clear(&mut self) {
        self.pending.clear();
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
        // A field with the caret takes every key, as the engine's Insert
        // will (plan M1 Deviation 5).
        if ctx.field.is_live() {
            self.pending.clear();
            return Resolved::Declined;
        }
        // An alt chord or a modified named key: no row names it, so what
        // was pending is dropped and the key goes on (`g` then ctrl+c
        // still quits).
        let Some(tok) = Tok::of(&ev) else {
            self.pending.clear();
            return Resolved::Declined;
        };
        self.step(ev, tok, ctx, true)
    }

    fn step(&mut self, ev: KeyEvent, tok: Tok, ctx: &KeyCtx, first: bool) -> Resolved {
        // 1. Esc clears what is pending, silently.
        if tok == Tok::Named(KeyCode::Esc) && !self.pending.is_idle() {
            self.pending.clear();
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
            Match::Complete(b) => {
                let count = self.pending.count();
                self.pending.clear();
                match b.intent {
                    Intent::Pass => Resolved::Declined,
                    _ => Resolved::Run(expand(b, count, ctx.cap)),
                }
            }
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
            // In vim a bare char on a navigation surface is vim's beep:
            // no component letter is left to reach (spec §3.4, plan M1
            // Deviation 1). Space is the surface's own key.
            let swallow = ctx.profile == KeyProfile::Vim
                && matches!(tok, Tok::Char(c) if c != ' ')
                && Surfaces::NAV.has(ctx.surface);
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
                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::PageUp | KeyCode::PageDown
                )
            ) {
                return Resolved::Run(vec![Step::Key(ev); n.min(ctx.cap)]);
            }
            return if first { self.step(ev, tok, ctx, false) } else { Resolved::Declined };
        }
        // A prefix and a key that does not extend it: vim's beep. A ctrl
        // chord is the exception: it goes on (`g` then ctrl+c quits).
        self.pending.clear();
        if matches!(tok, Tok::Ctrl(_)) { Resolved::Declined } else { Resolved::Consumed }
    }
}

/// A complete row's steps under its count rule.
fn expand(b: &table::Binding, count: Option<usize>, cap: usize) -> Vec<Step> {
    // A refused count runs nothing but its note (spec §3.5).
    if let (CountRule::Refuse(note), Some(_)) = (b.count, count) {
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
        // `{N}dd` hands N to the verb; a put repeats, capped by the
        // surface like a motion (plan M2 Deviation 10).
        Intent::Verb(v) if b.count == CountRule::Pass => {
            vec![Step::Action(Action::OnSelection(v.with_count(count.unwrap_or(1))))]
        }
        Intent::Verb(v) => vec![Step::Action(Action::OnSelection(v)); times(cap)],
        Intent::Note(text) => vec![note_step(text)],
        Intent::Pass => unreachable!("Pass is declined before expand"),
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
        KeyCtx { profile, surface, field: FieldCaret::None, user_bare: None, cap: MAX_COUNT }
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
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "j"), run(vec![key(KeyCode::Down)]));
        assert_eq!(feed(&mut Router::default(), &cx(Arrows, Surface::Sidebar), "j"), Resolved::Declined);
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &cx(Arrows, Surface::Sidebar), "3"), Resolved::Declined, "no counts in arrows");
        assert_eq!(r.echo(), "");
    }

    #[test]
    fn a_count_repeats_a_motion_capped_by_the_surface() {
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "3j"), run(vec![key(KeyCode::Down); 3]));
        let small = KeyCtx { cap: 2, ..cx(Vim, Surface::Sidebar) };
        assert_eq!(feed(&mut Router::default(), &small, "9j"), run(vec![key(KeyCode::Down); 2]));
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "3o"), run(vec![key(KeyCode::Enter)]), "o ignores it");
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
        assert_eq!(feed(&mut Router::default(), &resp, "G"), run(vec![ctrl_key(KeyCode::End)]));
        let mut ten = vec![ctrl_key(KeyCode::Home)];
        ten.extend(vec![key(KeyCode::Down); 9]);
        assert_eq!(feed(&mut Router::default(), &resp, "10G"), run(ten));
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
        assert_eq!(r.resolve(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), &s), Resolved::Declined);
        assert_eq!(r.echo(), "");
        feed(&mut r, &s, "3");
        assert_eq!(r.resolve(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT), &s), Resolved::Declined);
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
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "x"), Resolved::Consumed);
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Dialog), "x"), Resolved::Declined, "its letters answer");
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Other), "x"), Resolved::Declined);
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "<Space>"), Resolved::Declined, "Space is the surface's");
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
            feed(&mut r, &cx(Vim, Surface::Table(crate::components::editor::EditorTab::Params)), "j"),
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
    /// refuses one, `p` repeats (capped by the surface), shared letters
    /// once.
    #[test]
    fn vim_list_verbs_resolve_to_on_selection() {
        use crate::action::Verb;
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "dd"), run(vec![sel(Verb::Delete { count: 1 })]));
        assert_eq!(feed(&mut Router::default(), &s, "3dd"), run(vec![sel(Verb::Delete { count: 3 })]));
        assert_eq!(feed(&mut Router::default(), &s, "yy"), run(vec![sel(Verb::Yank)]));
        assert_eq!(
            feed(&mut Router::default(), &s, "3yy"),
            run(vec![Step::Action(Action::ShowToast("Counted yank is not supported".into(), ToastKind::Info))])
        );
        assert_eq!(feed(&mut Router::default(), &s, "2p"), run(vec![sel(Verb::Put { above: false }); 2]));
        let capped = KeyCtx { cap: 4, ..s.clone() };
        assert_eq!(feed(&mut Router::default(), &capped, "9P"), run(vec![sel(Verb::Put { above: true }); 4]));
        assert_eq!(feed(&mut Router::default(), &s, "a"), run(vec![sel(Verb::Add)]));
        assert_eq!(feed(&mut Router::default(), &s, "3r"), run(vec![sel(Verb::Rename)]), "a count is ignored");
        let mut r = Router::default();
        assert_eq!(feed(&mut r, &s, "d"), Resolved::Consumed);
        assert_eq!(r.echo(), "d");
    }

    /// Plan M2 Deviation 14: a counted `dd` is refused off the sidebar and
    /// the tables; one `dd` still deletes.
    #[test]
    fn a_counted_dd_is_refused_on_the_manage_and_variables_lists() {
        use crate::action::Verb;
        use crate::components::manage::ManageTab;
        let note = Step::Action(Action::ShowToast(
            "Counted delete works on requests and table rows".into(),
            ToastKind::Info,
        ));
        for surface in [Surface::ManageList(ManageTab::Environments), Surface::VarList, Surface::VarGrid] {
            let s = cx(Vim, surface);
            assert_eq!(feed(&mut Router::default(), &s, "2dd"), run(vec![note.clone()]), "{surface:?}");
            assert_eq!(feed(&mut Router::default(), &s, "dd"), run(vec![sel(Verb::Delete { count: 1 })]), "{surface:?}");
        }
    }

    /// Spec §4.5: the arrows profile's verbs are one letter each, and its
    /// old letters are gone.
    #[test]
    fn arrows_list_verbs_are_one_letter() {
        use crate::action::Verb;
        use crate::components::manage::ManageTab;
        let s = cx(Arrows, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "d"), run(vec![sel(Verb::Delete { count: 1 })]));
        assert_eq!(feed(&mut Router::default(), &s, "y"), run(vec![sel(Verb::Yank)]));
        assert_eq!(feed(&mut Router::default(), &s, "P"), run(vec![sel(Verb::Put { above: true })]));
        assert_eq!(feed(&mut Router::default(), &s, "a"), run(vec![sel(Verb::Add)]));
        assert_eq!(feed(&mut Router::default(), &s, "n"), Resolved::Declined, "n is no longer new");
        let env = cx(Arrows, Surface::ManageDetail(ManageTab::Environments));
        assert_eq!(feed(&mut Router::default(), &env, "t"), run(vec![sel(Verb::Tls)]));
        let form = cx(Arrows, Surface::VarForm);
        assert_eq!(feed(&mut Router::default(), &form, "r"), run(vec![sel(Verb::Reveal)]));
        assert_eq!(feed(&mut Router::default(), &form, "p"), run(vec![sel(Verb::Promote)]));
        let grid = cx(Arrows, Surface::VarGrid);
        assert_eq!(feed(&mut Router::default(), &grid, "c"), run(vec![sel(Verb::Yank)]));
        assert_eq!(feed(&mut Router::default(), &grid, "v"), run(vec![sel(Verb::Put { above: false })]));
        assert_eq!(
            feed(&mut Router::default(), &grid, "o"),
            run(vec![Step::Action(Action::StartNewOptionEdit)])
        );
    }

    /// Plan M2 Deviation 1: vim takes only the shared letters on the
    /// Variables form; its other verbs wait for M4's ex rows.
    #[test]
    fn vim_swallows_the_arrows_only_verb_letters() {
        for (surface, keys) in [
            (Surface::VarForm, "r"),
            (Surface::VarForm, "s"),
            (Surface::VarGrid, "e"),
            (Surface::VarGrid, "c"),
        ] {
            assert_eq!(feed(&mut Router::default(), &cx(Vim, surface), keys), Resolved::Consumed, "{surface:?} {keys}");
        }
    }

    /// Plan M2 Deviation 18 (amendment A5-1): a `keys.toml` letter wins
    /// over an arrows verb row, as it does over a vim one.
    #[test]
    fn an_arrows_keys_toml_letter_wins_over_a_verb_row() {
        let bound = KeyCtx { user_bare: Some(Action::Quit), ..cx(Arrows, Surface::Sidebar) };
        assert_eq!(feed(&mut Router::default(), &bound, "y"), run(vec![Step::Action(Action::Quit)]));
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
        assert_eq!(feed(&mut r, &cx(Vim, Surface::Sidebar), "3j"), run(vec![key(KeyCode::Down); 3]));
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
        assert_eq!(feed(&mut Router::default(), &resp, "0"), run(vec![key(KeyCode::Home)]));
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "0"), Resolved::Consumed);
    }

    #[test]
    fn undo_redo_and_the_palette_are_rows() {
        let s = cx(Vim, Surface::Sidebar);
        assert_eq!(feed(&mut Router::default(), &s, "u"), run(vec![Step::Action(Action::Undo)]));
        assert_eq!(feed(&mut Router::default(), &s, "3u"), run(vec![Step::Action(Action::Undo); 3]));
        assert_eq!(feed(&mut Router::default(), &s, "<C-r>"), run(vec![Step::Action(Action::Redo)]));
        assert_eq!(feed(&mut Router::default(), &s, "3:"), run(vec![Step::Action(Action::OpenPalette)]));
        assert_eq!(feed(&mut Router::default(), &cx(Vim, Surface::Picker), "u"), Resolved::Declined, "a picker types it");
    }

    #[test]
    fn notes_and_passes() {
        assert_eq!(
            feed(&mut Router::default(), &cx(Vim, Surface::Sidebar), "/"),
            run(vec![Step::Action(Action::ShowToast("No search in this list yet".into(), ToastKind::Info))])
        );
        let resp = cx(Vim, Surface::Response(crate::components::response::ViewMode::Pretty));
        assert_eq!(feed(&mut Router::default(), &resp, "/"), Resolved::Declined);
        assert_eq!(feed(&mut Router::default(), &resp, "3n"), Resolved::Declined);
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
                for b in table::TABLE.iter().filter(|b| b.profile.has(profile) && b.on.has(surface)) {
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
                        let still = matches!(table::lookup(&both, profile, surface), table::Match::Prefix);
                        assert_eq!(r.echo().is_empty(), !still, "{profile:?} {surface:?} {prefix:?} then {c:?}");
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
}
