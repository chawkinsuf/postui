//! Keycap spellings (piece 4 spec §4.6): a chip or a hint names the key
//! that runs its action in the active profile, read from the binding
//! table and the keymap, so it can never name a key the router does not
//! bind (plan M1 Deviation 3).

use super::ctx::{FieldSpell, SpellCtx};
use super::{notation, table};
use crate::action::Action;
use crate::config::KeyProfile;
use crate::keys::Keymap;

/// What a footer chip's keycap is.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    /// Spelled from what runs this action (`spell`). The chip's own action
    /// may be another, or none: the form's `u undo` hint.
    Of(Action),
    /// Two actions on one chip (`alt+←→ tabs`): both spelled, a shared
    /// prefix merged.
    Pair(Action, Action),
    /// A named key, or a chord the surface's own handler owns, the same in
    /// both profiles: `esc`, `enter`, `␣`, `alt+↑↓`, a picker's `↑↓`.
    Named(&'static str),
    /// A move pair on a navigation surface: `↑↓` or `j/k`, `←→` or `h/l`.
    Nav(Nav),
    /// The first of two actions that has a key here: the table's add-row
    /// chip is `a` on the table, and arrows' `alt+a` where the table's row
    /// does not apply (the url line's `SubFocus::None`, or while an
    /// existing row's cell edit is live); vim gave up `alt+a` (plan M2
    /// Deviations 4 and 5).
    OneOf(Action, Action),
    /// A dialog's answer letter: the same in both profiles.
    Answer(char),
    /// The send chip: `⇧enter` where the terminal reports it, else `^R`
    /// in arrows and the keymap's spelling in vim (Deviation 16).
    Send { shift_enter: bool },
    /// The quit chip: `q` in arrows where a plain `q` quits, else spelled
    /// (Deviation 16).
    Quit { plain_q: bool },
    /// A picker's highlight moves (spec §5.10): `↑↓` in arrows, `^N/^P` in
    /// vim, where the arrows still work too.
    Pick,
    /// A named key the surface owns in arrows, unless a table row spells
    /// `action` here: the selected prompt field's `enter edit` is vim's `o
    /// edit`, its `⇧enter confirm` vim's `enter confirm` (plan M4b
    /// Deviation 22).
    OrRow(&'static str, Action),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    UpDown,
    LeftRight,
}

/// The table row that runs `action` at `at`, if the router would look it
/// up there: the surface's rows with no field, the field class's rows in a
/// field's idle Normal (where `u` and `ctrl+r` are the surface's, handed
/// on by the engine), none while typing.
fn row_for(at: SpellCtx, action: &Action) -> Option<&'static str> {
    match at.field {
        FieldSpell::None => table::keys_for(at.profile, at.surface, action),
        FieldSpell::Normal(class) => table::keys_for_class(at.profile, class, action).or_else(|| {
            matches!(action, Action::Undo | Action::Redo)
                .then(|| table::keys_for(at.profile, at.surface, action))
                .flatten()
        }),
        FieldSpell::Typing => None,
    }
}

/// The key that runs `action` at `at`: the table's row first (the router
/// looks there before anything else), then the keymap's combo, then the
/// ex verb where `:` applies (spec §4.6; plan M4b Deviations 20 and 21).
/// `None` when nothing binds it.
pub fn spell(at: SpellCtx, action: &Action, keymap: &Keymap) -> Option<String> {
    if let Some(keys) = row_for(at, action) {
        return Some(notation::display(keys));
    }
    if let Some(combo) = keymap.combo_for_action(action) {
        return Some(combo);
    }
    colon_applies(at).then(|| super::ex::spelling(action)).flatten()
}

/// What the quit chip runs at `at`: `:q`'s gated quit where its keycap
/// reads `:q` (vim, where the line opens), so a click over a dialog meets
/// the dirty gate as the typed `:q` does; else `Quit`, whose keys are
/// arrows' `q` and the `^C` emergency exit.
pub fn quit_action(at: SpellCtx) -> Action {
    if at.profile == KeyProfile::Vim && colon_applies(at) { Action::GatedQuit } else { Action::Quit }
}

/// Which of a `Key::OneOf` pair its keycap is spelled from at `at`: `a`
/// where it spells, else `b`. A chip's click runs the same one.
pub fn one_of<'a>(at: SpellCtx, a: &'a Action, b: &'a Action, keymap: &Keymap) -> &'a Action {
    if spell(at, a, keymap).is_some() { a } else { b }
}

/// Whether the `:` line opens from `at`: its row applies there.
fn colon_applies(at: SpellCtx) -> bool {
    row_for(at, &Action::OpenExPalette).is_some()
}

/// The text `key` paints as at `at`, or `None` when the chip has no key
/// in this profile (it is then dropped).
pub fn keycap(key: &Key, at: SpellCtx, keymap: &Keymap) -> Option<String> {
    let vim = at.profile == KeyProfile::Vim;
    Some(match key {
        Key::Of(a) => glyphs(&spell(at, a, keymap)?),
        Key::Pair(a, b) => pair(&glyphs(&spell(at, a, keymap)?), &glyphs(&spell(at, b, keymap)?)),
        Key::Named(s) => (*s).to_string(),
        Key::Nav(Nav::UpDown) => if vim { "j/k" } else { "\u{2191}\u{2193}" }.to_string(),
        Key::Nav(Nav::LeftRight) => if vim { "h/l" } else { "\u{2190}\u{2192}" }.to_string(),
        Key::OneOf(a, b) => glyphs(&spell(at, one_of(at, a, b, keymap), keymap)?),
        Key::Answer(c) => c.to_string(),
        Key::Send { shift_enter: true } => "\u{21e7}enter".to_string(),
        Key::Send { shift_enter: false } if !vim => "^R".to_string(),
        Key::Send { .. } => glyphs(&spell(at, &Action::Send, keymap)?),
        Key::Quit { plain_q: true } if !vim => "q".to_string(),
        // `:q quit` in vim where the line opens (spec §4.6), spelled from
        // the ex verb the chip then runs; the keymap's `^C` on a picker or
        // a dialog.
        Key::Quit { .. } => match quit_action(at) {
            Action::GatedQuit => super::ex::spelling(&Action::GatedQuit)?,
            quit => glyphs(&spell(at, &quit, keymap)?),
        },
        Key::Pick => if vim { "^N/^P" } else { "\u{2191}\u{2193}" }.to_string(),
        Key::OrRow(named, a) => row_for(at, a).map_or_else(|| (*named).to_string(), notation::display),
    })
}

/// The painted `(keycap, label, action)` rows for `chips`, dropping the
/// chips with no key here.
pub fn resolve<L>(chips: Vec<(Key, L, Option<Action>)>, at: SpellCtx, keymap: &Keymap) -> Vec<(String, L, Option<Action>)> {
    chips
        .into_iter()
        .filter_map(|(k, l, a)| Some((keycap(&k, at, keymap)?, l, a)))
        .collect()
}

/// A combo's arrow names as the footer draws them: `alt+left` is `alt+←`.
pub(crate) fn glyphs(combo: &str) -> String {
    let (head, last) = match combo.rsplit_once('+') {
        Some((h, l)) => (format!("{h}+"), l),
        None => (String::new(), combo),
    };
    let last = match last {
        "left" => "\u{2190}",
        "right" => "\u{2192}",
        "up" => "\u{2191}",
        "down" => "\u{2193}",
        other => other,
    };
    format!("{head}{last}")
}

/// Two spellings on one chip: `alt+←` and `alt+→` merge to `alt+←→`;
/// anything else reads `a/b`.
pub(crate) fn pair(a: &str, b: &str) -> String {
    let arrows = ['\u{2190}', '\u{2192}', '\u{2191}', '\u{2193}'];
    match (a.char_indices().last(), b.char_indices().last()) {
        (Some((ia, ca)), Some((ib, cb)))
            if arrows.contains(&ca) && arrows.contains(&cb) && a[..ia] == b[..ib] =>
        {
            format!("{}{ca}{cb}", &a[..ia])
        }
        _ => format!("{a}/{b}"),
    }
}

/// The arrows profile's spelling of `chips` with the default keymap: how
/// the chip producers' tests read them.
#[cfg(test)]
pub fn spelled<L>(chips: Vec<(Key, L, Option<Action>)>) -> Vec<(String, L, Option<Action>)> {
    resolve(chips, SpellCtx::default(), &Keymap::default_bindings())
}

/// [`spelled`] on `surface`: a verb chip is spelled from the row on the
/// surface its pane is, so a test of a pane's chips names that surface.
#[cfg(test)]
pub fn spelled_on<L>(surface: crate::keyroute::Surface, chips: Vec<(Key, L, Option<Action>)>) -> Vec<(String, L, Option<Action>)> {
    resolve(chips, SpellCtx { surface, ..SpellCtx::default() }, &Keymap::default_bindings())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyroute::Surface;
    use crate::keyroute::ctx::FieldSpell;
    use KeyProfile::{Arrows, Vim};

    fn at(profile: KeyProfile, surface: Surface) -> SpellCtx {
        SpellCtx { profile, surface, field: FieldSpell::None }
    }

    /// Plan M4b Deviation 21: a screen field's idle Normal spells the field
    /// rows (`gT/gt`, `^W`, `ZZ`, `:`) and hands `u`/`^R` on; typing spells
    /// the keymap only.
    #[test]
    fn a_fields_idle_normal_spells_its_rows() {
        use crate::keyroute::FieldId;
        let v = Keymap::default_bindings_for(Vim);
        let class = table::field_class(FieldId::Url, Surface::UrlLine);
        let normal = SpellCtx { field: FieldSpell::Normal(class), ..at(Vim, Surface::UrlLine) };
        let typing = SpellCtx { field: FieldSpell::Typing, ..at(Vim, Surface::UrlLine) };
        let cap = |k: Key, s: SpellCtx| keycap(&k, s, &v);
        let tabs = || Key::Pair(Action::CycleTabs(-1), Action::CycleTabs(1));
        assert_eq!(cap(tabs(), normal).as_deref(), Some("gT/gt"));
        assert_eq!(cap(tabs(), typing), None);
        assert_eq!(cap(Key::Of(Action::Undo), normal).as_deref(), Some("u"), "the engine hands u on from idle Normal");
        assert_eq!(cap(Key::Of(Action::Undo), typing).as_deref(), Some("^Z"));
        assert_eq!(cap(Key::Of(Action::OpenExPalette), normal).as_deref(), Some(":"));
        assert_eq!(cap(Key::Of(Action::WriteQuit), normal).as_deref(), Some("ZZ"));
        assert_eq!(cap(Key::Of(Action::SaveRequest), normal).as_deref(), Some(":w"), "the line opens from idle Normal");
        assert_eq!(cap(Key::Of(Action::SaveRequest), typing), None);
        assert_eq!(cap(Key::Of(Action::OnSelection(crate::action::Verb::Delete { count: 1 })), normal), None, "no verb row in a field");
        let prompt_class = table::field_class(FieldId::ModalInput(0), Surface::Prompt);
        let prompt = SpellCtx { field: FieldSpell::Normal(prompt_class), ..at(Vim, Surface::Prompt) };
        assert_eq!(cap(tabs(), prompt), None, "a dialog field has no tab rows");
    }

    /// Plan M4b Deviation 22: a named key in arrows, a row in vim.
    #[test]
    fn or_row_is_the_named_key_unless_a_row_spells_the_action() {
        let v = Keymap::default_bindings_for(Vim);
        let a = Keymap::default_bindings();
        let edit = Key::OrRow("enter", Action::ReopenField);
        let confirm = Key::OrRow("\u{21e7}enter", Action::ConfirmTopModal);
        assert_eq!(keycap(&edit, at(Arrows, Surface::Prompt), &a).as_deref(), Some("enter"));
        assert_eq!(keycap(&edit, at(Vim, Surface::Prompt), &v).as_deref(), Some("o"));
        assert_eq!(keycap(&confirm, at(Arrows, Surface::Prompt), &a).as_deref(), Some("\u{21e7}enter"));
        assert_eq!(keycap(&confirm, at(Vim, Surface::Prompt), &v).as_deref(), Some("enter"));
        assert_eq!(keycap(&confirm, at(Vim, Surface::Form), &v).as_deref(), Some("\u{21e7}enter"), "a form keeps its ladder");
    }

    /// Spec §5.10: the footer advertises `^N/^P` on a picker in vim.
    #[test]
    fn a_pickers_move_keys_per_profile() {
        let keymap = Keymap::default_bindings();
        let at = |profile| SpellCtx { profile, surface: Surface::Picker, field: FieldSpell::Typing };
        assert_eq!(keycap(&Key::Pick, at(Arrows), &keymap).as_deref(), Some("\u{2191}\u{2193}"));
        assert_eq!(keycap(&Key::Pick, at(Vim), &keymap).as_deref(), Some("^N/^P"));
    }

    #[test]
    fn a_chip_is_spelled_from_what_runs_it() {
        let a = Keymap::default_bindings();
        let v = Keymap::default_bindings_for(Vim);
        let cap = |k: Key, s: SpellCtx, m: &Keymap| keycap(&k, s, m);
        assert_eq!(cap(Key::Of(Action::Undo), at(Arrows, Surface::Sidebar), &a).as_deref(), Some("^Z"));
        assert_eq!(cap(Key::Of(Action::Undo), at(Vim, Surface::Sidebar), &v).as_deref(), Some("u"));
        let live = SpellCtx { field: FieldSpell::Typing, ..at(Vim, Surface::UrlLine) };
        assert_eq!(cap(Key::Of(Action::Undo), live, &v).as_deref(), Some("^Z"), "a live field types u");
        assert_eq!(cap(Key::Of(Action::OpenPalette), at(Arrows, Surface::Sidebar), &a).as_deref(), Some("^P"));
        assert_eq!(cap(Key::Of(Action::OpenPalette), at(Vim, Surface::Sidebar), &v).as_deref(), Some("^P"));
        assert_eq!(cap(Key::OneOf(Action::OpenExPalette, Action::OpenPalette), at(Vim, Surface::Sidebar), &v).as_deref(), Some(":"));
        assert_eq!(cap(Key::OneOf(Action::OpenExPalette, Action::OpenPalette), at(Arrows, Surface::Sidebar), &a).as_deref(), Some("^P"));
        let tabs = Key::Pair(Action::CycleTabs(-1), Action::CycleTabs(1));
        let alt = crate::keys::alt_label();
        assert_eq!(cap(tabs.clone(), at(Arrows, Surface::Sidebar), &a), Some(format!("{alt}+\u{2190}\u{2192}")));
        assert_eq!(cap(tabs.clone(), at(Vim, Surface::Sidebar), &v).as_deref(), Some("gT/gt"), "plan M4a: the rows");
        assert_eq!(cap(tabs, live, &v), None, "vim's keymap binds no alt+\u{2190}/\u{2192}: the chip drops in a field");
        let del = Key::Of(Action::OnSelection(crate::action::Verb::Delete { count: 1 }));
        assert_eq!(cap(del.clone(), at(Arrows, Surface::Sidebar), &a).as_deref(), Some("d"));
        assert_eq!(cap(del.clone(), at(Vim, Surface::Sidebar), &v).as_deref(), Some("dd"));
        assert_eq!(cap(del, at(Vim, Surface::Settings), &v), None, "no row there: the chip drops");
        let add = || Key::OneOf(Action::OnSelection(crate::action::Verb::Add), Action::TableAddRow);
        let table = Surface::Table(crate::components::editor::EditorTab::Headers);
        assert_eq!(cap(add(), at(Arrows, table), &a).as_deref(), Some("a"));
        assert_eq!(cap(add(), at(Vim, table), &v).as_deref(), Some("a"));
        let alt = crate::keys::alt_label();
        assert_eq!(
            cap(add(), at(Arrows, Surface::UrlLine), &a),
            Some(format!("{alt}+a")),
            "at SubFocus::None (the url line): the chord"
        );
        assert_eq!(cap(add(), at(Vim, Surface::UrlLine), &v), None, "vim gave up alt+a (plan M2 Deviation 5)");
        assert_eq!(cap(Key::Nav(Nav::UpDown), at(Vim, Surface::Settings), &v).as_deref(), Some("j/k"));
        assert_eq!(cap(Key::Quit { plain_q: true }, at(Arrows, Surface::Sidebar), &a).as_deref(), Some("q"));
        assert_eq!(cap(Key::Quit { plain_q: true }, at(Vim, Surface::Sidebar), &v).as_deref(), Some(":q"));
        assert_eq!(cap(Key::Send { shift_enter: false }, at(Arrows, Surface::UrlLine), &a).as_deref(), Some("^R"));
        assert_eq!(cap(Key::Send { shift_enter: false }, at(Vim, Surface::UrlLine), &v).as_deref(), Some("^Enter"));
        assert_eq!(cap(Key::Send { shift_enter: true }, at(Vim, Surface::UrlLine), &v).as_deref(), Some("\u{21e7}enter"));
    }

    /// Spec §4.6 (plan M4b Deviation 20): where no key binds an action in
    /// vim, its ex verb spells it; the quit chip reads `:q` where `:`
    /// applies and the keymap's `^C` elsewhere; typing spells the keymap only.
    #[test]
    fn ex_verbs_spell_what_no_key_binds() {
        let v = Keymap::default_bindings_for(Vim);
        let a = Keymap::default_bindings();
        let cap = |k: Key, s: SpellCtx, m: &Keymap| keycap(&k, s, m);
        assert_eq!(cap(Key::Of(Action::SaveRequest), at(Vim, Surface::Sidebar), &v).as_deref(), Some(":w"));
        assert_eq!(cap(Key::Of(Action::SaveRequest), at(Arrows, Surface::Sidebar), &a).as_deref(), Some("^S"));
        assert_eq!(cap(Key::Of(Action::CycleSplit), at(Vim, Surface::UrlLine), &v).as_deref(), Some(":split"));
        assert_eq!(cap(Key::Of(Action::Send), at(Vim, Surface::UrlLine), &v).as_deref(), Some("^Enter"), "a bound chord beats the verb");
        assert_eq!(cap(Key::Of(Action::SaveRequest), at(Vim, Surface::Picker), &v), None, "no : on a picker");
        assert_eq!(cap(Key::Quit { plain_q: false }, at(Vim, Surface::Sidebar), &v).as_deref(), Some(":q"));
        assert_eq!(cap(Key::Quit { plain_q: false }, at(Vim, Surface::Picker), &v).as_deref(), Some("^C"));
        assert_eq!(cap(Key::Quit { plain_q: true }, at(Arrows, Surface::Sidebar), &a).as_deref(), Some("q"));
        let live = SpellCtx { field: FieldSpell::Typing, ..at(Vim, Surface::UrlLine) };
        assert_eq!(cap(Key::Of(Action::SaveRequest), live, &v), None, "typing: the keymap only");
    }

    #[test]
    fn pairs_merge_only_arrow_glyphs() {
        assert_eq!(pair("alt+\u{2190}", "alt+\u{2192}"), "alt+\u{2190}\u{2192}");
        assert_eq!(pair("gT", "gt"), "gT/gt");
        assert_eq!(glyphs("alt+right"), "alt+\u{2192}");
        assert_eq!(glyphs("^P"), "^P");
    }
}
