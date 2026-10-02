//! The binding table (piece 4 spec §3.5): one static table for both
//! profiles and every surface. A key's meaning is a lookup here; what a
//! prefix is follows from the rows, so no separate prefix list can drift.
//! M1's rows are the vim profile's list, table, response and global keys
//! (the motions, `u`, `ctrl+r`, `:`); the arrows profile has no rows yet
//! (its letters are M2's shared verbs).

use super::ctx::Surface;
use super::notation::{self, Tok};
use crate::action::Action;
use crate::config::KeyProfile;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::OnceLock;

/// A named key a row replays, with or without ctrl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Named {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl Named {
    pub const fn key(code: KeyCode) -> Self {
        Self { code, ctrl: false }
    }

    pub const fn ctrl(code: KeyCode) -> Self {
        Self { code, ctrl: true }
    }

    pub fn event(self) -> KeyEvent {
        let mods = if self.ctrl {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(self.code, mods)
    }
}

/// What a complete row does.
#[derive(Debug, Clone, Copy)]
pub enum Intent {
    /// Replay a named key through today's routing: `j` is Down.
    Key(Named),
    /// `gg` and `G`: with no count, `bare`; with a count N, `home` then
    /// N−1 Downs (row N, clamped by the surface).
    Edge { bare: Named, home: Named },
    /// A fixed action.
    Act(fn() -> Action),
    /// A short note (an Info toast, plan M1 Deviation 9); nothing else.
    Note(&'static str),
    /// The key goes on as typed: the surface's own arm already does what
    /// the vim key means (plan M1 Deviation 8).
    Pass,
}

/// How a count applies (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountRule {
    /// The intent runs N times.
    Repeat,
    /// It runs once; the count is dropped.
    Ignore,
    /// The intent reads N itself (`Edge`).
    Pass,
}

/// Which profiles a row binds in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profiles(u8);

impl Profiles {
    pub const VIM: Self = Self(2);

    pub fn has(self, p: KeyProfile) -> bool {
        let bit = match p {
            KeyProfile::Arrows => 1,
            KeyProfile::Vim => 2,
        };
        self.0 & bit != 0
    }
}

/// A set of surface kinds (the data a `Surface` carries is ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surfaces(u32);

impl Surfaces {
    pub const SIDEBAR: Self = Self(1 << 0);
    pub const URL_LINE: Self = Self(1 << 1);
    pub const METHOD: Self = Self(1 << 2);
    pub const TABLE: Self = Self(1 << 3);
    pub const BODY: Self = Self(1 << 4);
    pub const RESPONSE: Self = Self(1 << 5);
    pub const VAR_LIST: Self = Self(1 << 6);
    pub const VAR_GRID: Self = Self(1 << 7);
    pub const VAR_FORM: Self = Self(1 << 8);
    pub const MANAGE_LIST: Self = Self(1 << 9);
    pub const MANAGE_DETAIL: Self = Self(1 << 10);
    pub const SETTINGS: Self = Self(1 << 11);
    pub const PROMPT: Self = Self(1 << 12);
    pub const FORM: Self = Self(1 << 13);
    pub const DIALOG: Self = Self(1 << 14);
    pub const PICKER: Self = Self(1 << 15);
    pub const OTHER: Self = Self(1 << 16);

    /// Every surface whose rows or controls the keyboard walks: vim's
    /// motions bind here, and in vim a bare char with no row is
    /// swallowed here (spec §3.4 step 4).
    pub const NAV: Self = Self::SIDEBAR
        .or(Self::URL_LINE)
        .or(Self::METHOD)
        .or(Self::TABLE)
        .or(Self::RESPONSE)
        .or(Self::VAR_LIST)
        .or(Self::VAR_GRID)
        .or(Self::VAR_FORM)
        .or(Self::MANAGE_LIST)
        .or(Self::MANAGE_DETAIL)
        .or(Self::SETTINGS);
    /// The navigation surfaces but the response, whose `gg`/`G` are
    /// ctrl+Home/ctrl+End (its bare Home/End scroll sideways).
    pub const LISTS: Self = Self::NAV.minus(Self::RESPONSE);
    /// Where a row opens on Enter (`o`).
    pub const OPENS_ROW: Self = Self::SIDEBAR
        .or(Self::TABLE)
        .or(Self::VAR_LIST)
        .or(Self::VAR_GRID)
        .or(Self::MANAGE_LIST);
    /// Where a field opens on Enter (`i`). Manage detail panes take it
    /// too (spec §5.4; plan M1 Deviation 24).
    pub const OPENS_FIELD: Self = Self::TABLE
        .or(Self::VAR_GRID)
        .or(Self::VAR_FORM)
        .or(Self::SETTINGS)
        .or(Self::URL_LINE)
        .or(Self::MANAGE_DETAIL);
    /// The lists vim's `/` has no search on yet (key list §6, parked).
    pub const SEARCHLESS: Self = Self::OPENS_ROW;
    /// Where `u`, `ctrl+r` and `:` are the app's (spec §5.11): everywhere
    /// but pickers (they type) and dialogs (their letters answer).
    pub const APP: Self = Self::NAV
        .or(Self::BODY)
        .or(Self::PROMPT)
        .or(Self::FORM)
        .or(Self::OTHER);

    pub const fn or(self, o: Self) -> Self {
        Self(self.0 | o.0)
    }

    pub const fn minus(self, o: Self) -> Self {
        Self(self.0 & !o.0)
    }

    pub const fn of(s: Surface) -> Self {
        match s {
            Surface::Sidebar => Self::SIDEBAR,
            Surface::UrlLine => Self::URL_LINE,
            Surface::Method => Self::METHOD,
            Surface::Table(_) => Self::TABLE,
            Surface::Body => Self::BODY,
            Surface::Response(_) => Self::RESPONSE,
            Surface::VarList => Self::VAR_LIST,
            Surface::VarGrid => Self::VAR_GRID,
            Surface::VarForm => Self::VAR_FORM,
            Surface::ManageList(_) => Self::MANAGE_LIST,
            Surface::ManageDetail(_) => Self::MANAGE_DETAIL,
            Surface::Settings => Self::SETTINGS,
            Surface::Prompt => Self::PROMPT,
            Surface::Form => Self::FORM,
            Surface::Dialog => Self::DIALOG,
            Surface::Picker => Self::PICKER,
            Surface::Other => Self::OTHER,
        }
    }

    pub fn has(self, s: Surface) -> bool {
        self.0 & Self::of(s).0 != 0
    }
}

/// One row: these keys, in these profiles, on these surfaces, mean this.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    /// Vim notation: `j`, `gg`, `<C-r>`.
    pub keys: &'static str,
    pub profile: Profiles,
    pub on: Surfaces,
    pub intent: Intent,
    pub count: CountRule,
}

const fn row(keys: &'static str, on: Surfaces, intent: Intent, count: CountRule) -> Binding {
    Binding {
        keys,
        profile: Profiles::VIM,
        on,
        intent,
        count,
    }
}

const DOWN: Named = Named::key(KeyCode::Down);
const HOME: Named = Named::key(KeyCode::Home);

fn undo() -> Action {
    Action::Undo
}

fn redo() -> Action {
    Action::Redo
}

fn palette() -> Action {
    Action::OpenPalette
}

/// Every row. Order does not matter: no complete row is a prefix of
/// another on a shared surface (`no_two_rows_collide`).
pub static TABLE: &[Binding] = &[
    // Key list §6 and §7: moving.
    row("j", Surfaces::NAV, Intent::Key(DOWN), CountRule::Repeat),
    row("k", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Up)), CountRule::Repeat),
    row("h", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Left)), CountRule::Repeat),
    row("l", Surfaces::NAV, Intent::Key(Named::key(KeyCode::Right)), CountRule::Repeat),
    row("gg", Surfaces::LISTS, Intent::Edge { bare: HOME, home: HOME }, CountRule::Pass),
    row("G", Surfaces::LISTS, Intent::Edge { bare: Named::key(KeyCode::End), home: HOME }, CountRule::Pass),
    row(
        "gg",
        Surfaces::RESPONSE,
        Intent::Edge { bare: Named::ctrl(KeyCode::Home), home: Named::ctrl(KeyCode::Home) },
        CountRule::Pass,
    ),
    row(
        "G",
        Surfaces::RESPONSE,
        Intent::Edge { bare: Named::ctrl(KeyCode::End), home: Named::ctrl(KeyCode::Home) },
        CountRule::Pass,
    ),
    row("0", Surfaces::RESPONSE, Intent::Key(HOME), CountRule::Ignore),
    row("$", Surfaces::RESPONSE, Intent::Key(Named::key(KeyCode::End)), CountRule::Ignore),
    // Key list §6: opening.
    row("o", Surfaces::OPENS_ROW, Intent::Key(Named::key(KeyCode::Enter)), CountRule::Ignore),
    row("i", Surfaces::OPENS_FIELD, Intent::Key(Named::key(KeyCode::Enter)), CountRule::Ignore),
    // Key list §7: the response's own search keys go on as typed.
    row("/", Surfaces::RESPONSE, Intent::Pass, CountRule::Ignore),
    row("n", Surfaces::RESPONSE, Intent::Pass, CountRule::Ignore),
    row("N", Surfaces::RESPONSE, Intent::Pass, CountRule::Ignore),
    // Key list §6, Out (parked by the user): a list has no search yet.
    row("/", Surfaces::SEARCHLESS, Intent::Note("No search in this list yet"), CountRule::Ignore),
    // Key list §6 and §9: the app's undo, redo and command palette.
    row("u", Surfaces::APP, Intent::Act(undo), CountRule::Repeat),
    row("<C-r>", Surfaces::APP, Intent::Act(redo), CountRule::Repeat),
    row(":", Surfaces::APP, Intent::Act(palette), CountRule::Ignore),
];

/// Each row's keys, parsed once.
fn parsed() -> &'static [Vec<Tok>] {
    static PARSED: OnceLock<Vec<Vec<Tok>>> = OnceLock::new();
    PARSED.get_or_init(|| TABLE.iter().map(|b| notation::parse(b.keys)).collect())
}

/// What the buffered keys are on a surface.
#[derive(Debug, Clone, Copy)]
pub enum Match {
    Complete(&'static Binding),
    /// Some row on this surface extends the buffer.
    Prefix,
    NoMatch,
}

pub fn lookup(buf: &[Tok], profile: KeyProfile, surface: Surface) -> Match {
    let mut prefix = false;
    for (b, keys) in TABLE.iter().zip(parsed()) {
        if !b.profile.has(profile) || !b.on.has(surface) {
            continue;
        }
        if keys.as_slice() == buf {
            return Match::Complete(b);
        }
        if keys.len() > buf.len() && keys.starts_with(buf) {
            prefix = true;
        }
    }
    if prefix { Match::Prefix } else { Match::NoMatch }
}

/// The keys of the row that runs `action` here, for keycaps (spec §4.6).
pub fn keys_for(profile: KeyProfile, surface: Surface, action: &Action) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|b| {
            b.profile.has(profile)
                && b.on.has(surface)
                && matches!(b.intent, Intent::Act(f) if f() == *action)
        })
        .map(|b| b.keys)
}

/// The chars some vim row starts with: a `keys.toml` binding of one of
/// them shadows vim's key (spec §4.4, plan M1 Deviation 19).
pub fn vim_first_chars() -> Vec<char> {
    let mut out: Vec<char> = TABLE
        .iter()
        .zip(parsed())
        .filter(|(b, _)| b.profile.has(KeyProfile::Vim))
        .filter_map(|(_, keys)| match keys.first() {
            Some(Tok::Char(c)) => Some(*c),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a digit builds a count on `surface` in the vim profile: some
/// row there repeats or reads one (plan M1 Deviation 22). Derived from
/// [`TABLE`], so it cannot drift from the rows.
pub fn takes_count(surface: Surface) -> bool {
    TABLE.iter().any(|b| {
        b.profile.has(KeyProfile::Vim)
            && b.on.has(surface)
            && matches!(b.count, CountRule::Repeat | CountRule::Pass)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_on(profile: KeyProfile, surface: Surface) -> Vec<(&'static Binding, &'static Vec<Tok>)> {
        TABLE
            .iter()
            .zip(parsed())
            .filter(|(b, _)| b.profile.has(profile) && b.on.has(surface))
            .collect()
    }

    /// Spec §3.4: in the arrows profile the machine never leaves Idle.
    #[test]
    fn arrows_rows_are_one_key_long() {
        for (b, keys) in TABLE.iter().zip(parsed()) {
            if b.profile.has(KeyProfile::Arrows) {
                assert_eq!(keys.len(), 1, "{}", b.keys);
            }
        }
    }

    #[test]
    fn no_two_rows_collide() {
        for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
            for surface in Surface::SAMPLES {
                let rows = rows_on(profile, surface);
                for (i, (a, ka)) in rows.iter().enumerate() {
                    for (b, kb) in &rows[i + 1..] {
                        assert_ne!(ka, kb, "{} twice on {surface:?}", a.keys);
                        assert!(
                            !kb.starts_with(ka) && !ka.starts_with(kb),
                            "{} and {} on {surface:?}: a complete row is a prefix of another",
                            a.keys,
                            b.keys
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn counts_fit_their_intents() {
        for b in TABLE {
            let edge = matches!(b.intent, Intent::Edge { .. });
            assert_eq!(edge, b.count == CountRule::Pass, "{}: Pass is for Edge rows", b.keys);
            if matches!(b.intent, Intent::Pass | Intent::Note(_)) {
                assert_eq!(b.count, CountRule::Ignore, "{}", b.keys);
            }
        }
    }

    #[test]
    fn lookups_find_rows_prefixes_and_misses() {
        use KeyProfile::Vim;
        assert!(matches!(lookup(&[Tok::Char('j')], Vim, Surface::Sidebar), Match::Complete(b) if b.keys == "j"));
        assert!(matches!(lookup(&[Tok::Char('g')], Vim, Surface::Sidebar), Match::Prefix));
        assert!(matches!(lookup(&[Tok::Char('x')], Vim, Surface::Sidebar), Match::NoMatch));
        assert!(matches!(lookup(&[Tok::Char('j')], KeyProfile::Arrows, Surface::Sidebar), Match::NoMatch));
        assert!(matches!(lookup(&[Tok::Char('j')], Vim, Surface::Picker), Match::NoMatch), "a picker types");
        assert_eq!(keys_for(Vim, Surface::Sidebar, &Action::Undo), Some("u"));
        assert_eq!(keys_for(Vim, Surface::Picker, &Action::Undo), None);
        assert!(vim_first_chars().contains(&'g'));
    }

    #[test]
    fn i_opens_the_field_on_a_manage_detail_pane() {
        use crate::components::manage::ManageTab;
        let detail = Surface::ManageDetail(ManageTab::Environments);
        assert!(
            matches!(lookup(&[Tok::Char('i')], KeyProfile::Vim, detail), Match::Complete(b) if b.keys == "i")
        );
    }

    #[test]
    fn counts_are_taken_where_a_row_takes_one() {
        assert!(takes_count(Surface::Sidebar), "j repeats on a list");
        assert!(takes_count(Surface::Response(crate::components::response::ViewMode::Pretty)));
        assert!(!takes_count(Surface::Dialog), "a dialog's letters answer; digits are not counts");
        assert!(!takes_count(Surface::Picker), "a picker types");
    }
}
