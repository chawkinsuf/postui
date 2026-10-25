//! The `:` command line's verbs (piece 4 spec §5.9): one table of
//! `(names, run, caption)` rows. `:` opens the palette in ex mode
//! (`Action::OpenExPalette`); the palette re-resolves its whole query here
//! on every keystroke and pins the result as row 0, so the list always
//! shows what Enter runs. A list verb runs as `Action::OnSelection`, which
//! the app resolves against the surface under the line (spec §6.1), and
//! pins only when that surface resolves it now (`Scope`).

use crate::action::{Action, Verb};
use crate::keyroute::Surface;

/// What a verb runs.
#[derive(Debug, Clone, Copy)]
pub enum Run {
    Act(fn() -> Action),
    /// Two fixed actions in order (`:e!`: discard, then reload).
    Then(fn() -> Action, fn() -> Action),
    /// A list verb on the surface under the line (spec §6.1).
    Verb(Verb),
}

/// One verb: every spelling (the full name first), what it runs, the
/// pinned row's title, and the shortest prefix that reaches it.
#[derive(Debug, Clone, Copy)]
pub struct ExVerb {
    pub names: &'static [&'static str],
    pub run: Run,
    pub caption: &'static str,
    pub least: usize,
}

const fn verb(names: &'static [&'static str], run: Run, caption: &'static str) -> ExVerb {
    ExVerb { names, run, caption, least: 1 }
}

impl ExVerb {
    /// A prefix shorter than `least` does not reach this verb, as Vim's
    /// `:imp[ort]`: the shorter ones are Vim's own commands (`:c` change,
    /// `:co` copy, `:i` insert, `:im` imap).
    const fn least(self, least: usize) -> ExVerb {
        ExVerb { least, ..self }
    }
}

use act::*;

/// What the `Run::Act` rows run, one per line beside the table.
#[rustfmt::skip]
mod act {
    use crate::action::Action;

    pub fn save() -> Action { Action::SaveRequest }
    pub fn close() -> Action { Action::CloseWindow { write: false, force: false } }
    pub fn force_close() -> Action { Action::CloseWindow { write: false, force: true } }
    pub fn write_close() -> Action { Action::CloseWindow { write: true, force: false } }
    pub fn force_write_close() -> Action { Action::CloseWindow { write: true, force: true } }
    pub fn quit() -> Action { Action::GatedQuit }
    pub fn force_quit() -> Action { Action::ForceQuit }
    pub fn write_quit() -> Action { Action::WriteQuit }
    pub fn force_write() -> Action { Action::ForceWrite { quit: false } }
    pub fn force_write_quit() -> Action { Action::ForceWrite { quit: true } }
    pub fn reload() -> Action { Action::ReloadFromDisk }
    pub fn discard() -> Action { Action::DiscardChanges }
    pub fn send() -> Action { Action::Send }
    pub fn project() -> Action { Action::OpenProjectChooser }
    pub fn manage() -> Action { Action::OpenManage { tab: None } }
    pub fn theme() -> Action { Action::OpenThemeChooser }
    pub fn split() -> Action { Action::CycleSplit }
    pub fn split_back() -> Action { Action::CycleSplitBack }
    pub fn nohl() -> Action { Action::NoHlSearch }
    pub fn import_curl() -> Action { Action::ImportCurl { folder: None } }
    pub fn copy_curl() -> Action { Action::CopyAsCurl(None) }
}

/// Every verb, in the order a prefix tie is broken (Deviation 8): the
/// file verbs, the request verbs, the app commands, the Variable Manager's.
#[rustfmt::skip]
pub static VERBS: &[ExVerb] = &[
    verb(&["write", "w", "update", "up", "wa"], Run::Act(save), "Save request"),
    // The window verbs quit on the main screen and close Manage; their `a`
    // forms quit everywhere (`Action::CloseWindow`).
    verb(&["quit", "q"], Run::Act(close), "Quit"),
    verb(&["qall", "qa", "quitall"], Run::Act(quit), "Quit"),
    verb(&["quit!", "q!"], Run::Act(force_close), "Quit without saving"),
    verb(&["qall!", "qa!", "quitall!"], Run::Act(force_quit), "Quit without saving"),
    verb(&["wq", "x", "xit", "exit"], Run::Act(write_close), "Save request, then quit"),
    verb(&["wqall", "wqa", "xall", "xa"], Run::Act(write_quit), "Save request, then quit"),
    verb(&["write!", "w!", "update!", "up!", "wa!"], Run::Act(force_write), "Save request over outside changes"),
    verb(&["wq!", "x!", "xit!", "exit!"], Run::Act(force_write_close), "Save request over outside changes, then quit"),
    verb(&["wqall!", "wqa!", "xall!", "xa!"], Run::Act(force_write_quit), "Save request over outside changes, then quit"),
    verb(&["edit", "e"], Run::Act(reload), "Reload from disk"),
    verb(&["edit!", "e!"], Run::Then(discard, reload), "Discard changes, then reload"),
    verb(&["send"], Run::Act(send), "Send request"),
    verb(&["new"], Run::Verb(Verb::Add), "New…"),
    verb(&["rename"], Run::Verb(Verb::Rename), "Rename…"),
    verb(&["delete", "del"], Run::Verb(Verb::Delete { count: 1 }), "Delete"),
    verb(&["move"], Run::Verb(Verb::Move), "Move…"),
    // Vim 9.1's `:imp[ort]`; `:co` up to `:copy` are Vim's `:copy`.
    verb(&["import"], Run::Act(import_curl), "Import curl from clipboard").least(3),
    verb(&["copycurl"], Run::Act(copy_curl), "Copy request as curl").least(5),
    verb(&["project"], Run::Act(project), "Choose project…"),
    verb(&["manage"], Run::Act(manage), "Manage"),
    verb(&["theme"], Run::Act(theme), "Change theme…"),
    verb(&["split"], Run::Act(split), "Cycle split"),
    verb(&["splitback"], Run::Act(split_back), "Cycle split back"),
    verb(&["nohlsearch", "noh"], Run::Act(nohl), "Clear the search highlight"),
    verb(&["group"], Run::Verb(Verb::AddAlt), "New selector…"),
    verb(&["value"], Run::Verb(Verb::EditValue), "Edit option…"),
    verb(&["fields"], Run::Verb(Verb::Fields), "Group fields…"),
    verb(&["promote"], Run::Verb(Verb::Promote), "Promote variable…"),
    verb(&["secret"], Run::Verb(Verb::Secret), "Toggle secret"),
    verb(&["clear"], Run::Verb(Verb::ClearValue), "Clear environment value"),
    verb(&["reveal"], Run::Verb(Verb::Reveal), "Reveal secret"),
    verb(&["tls"], Run::Verb(Verb::Tls), "Cycle TLS mode"),
];

/// Vim commands we do not run (spec §5.9, key list X.unsupported): their
/// exact spellings. A query that starts a range is refused the same way.
/// `:u` and `:red` are Vim's undo and redo: listed so `:u` never reaches
/// `:up` by prefix and saves instead.
#[rustfmt::skip]
pub const UNSUPPORTED: &[&str] = &[
    "s", "substitute", "g", "global", "v", "vglobal", "se", "set", "sor", "sort", "norm", "normal", "r", "read", "!",
    "u", "undo", "red", "redo",
];

/// What the line can act on (Deviation 10): the surface under it, and the
/// list verbs `App::selection_action` resolves there now. Read once when
/// `:` opens the line; the cursor cannot move while it is up.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub surface: Surface,
    pub verbs: Vec<Verb>,
}

/// The pinned row: the verb's full name, its title, and what Enter runs.
/// `CloseField` leads, so a field parked under the line is committed
/// first (spec §5.9); it is a no-op when nothing is open. `:q!` and `:qa!`
/// have none: they abandon the field.
#[derive(Debug, Clone, PartialEq)]
pub struct ExRow {
    pub name: &'static str,
    pub caption: &'static str,
    pub actions: Vec<Action>,
}

/// What a query resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Pinned(ExRow),
    /// Runs nothing: the one muted row's text (`Not supported: :s`,
    /// `Nothing to :fields here`). The palette shows no fuzzy rows under it.
    Refused(String),
    /// No verb: the palette's fuzzy rows alone.
    Nothing,
}

fn row(v: &ExVerb) -> ExRow {
    // `:q!` and `:qa!` abandon what the field holds, as Vim's abandon a
    // changed buffer (Vim 9.1: the text is dropped, nothing is written), so
    // no close commits it first, and one that would refuse cannot stop them.
    let abandons = matches!(v.run, Run::Act(f) if matches!(f(), Action::ForceQuit | Action::CloseWindow { write: false, force: true }));
    let mut actions = if abandons { Vec::new() } else { vec![Action::CloseField] };
    match v.run {
        Run::Act(f) => actions.push(f()),
        Run::Then(a, b) => {
            actions.push(a());
            actions.push(b());
        }
        Run::Verb(verb) => actions.push(Action::OnSelection(verb)),
    }
    ExRow { name: v.names[0], caption: v.caption, actions }
}

fn runs_in(v: &ExVerb, scope: &Scope) -> bool {
    match v.run {
        Run::Verb(verb) => scope.verbs.iter().any(|w| w.kind() == verb.kind()),
        Run::Act(_) | Run::Then(..) => true,
    }
}

/// The shortest of `v`'s names that `query` starts, when the query is at
/// least `v.least` long.
fn shortest_start(v: &ExVerb, query: &str) -> Option<usize> {
    if query.len() < v.least {
        return None;
    }
    v.names.iter().filter(|n| n.starts_with(query)).map(|n| n.len()).min()
}

/// The verb `query` spells among those `keep` admits (Deviation 8): an
/// exact spelling, else the shortest name the query starts, a tie going
/// to the earlier row.
fn find(query: &str, keep: impl Fn(&ExVerb) -> bool) -> Option<&'static ExVerb> {
    let kept = || VERBS.iter().filter(|v| keep(v));
    kept().find(|v| v.names.contains(&query)).or_else(|| {
        kept()
            .filter_map(|v| shortest_start(v, query).map(|len| (len, v)))
            .min_by_key(|(len, _)| *len) // the first of equal minimums
            .map(|(_, v)| v)
    })
}

/// The verb `query` names (Deviation 8): refused spellings and ranges
/// first; then an exact spelling; then the shortest name the query starts.
/// Only verbs that run in `scope` count; a verb that would match but
/// does not run here is refused with its own text (Deviation 9). Spaces
/// around the query are not part of it, and a verb given an argument
/// (`:w file`, `:e path`) is refused: none of ours takes one.
pub fn resolve(query: &str, scope: &Scope) -> Resolved {
    let query = query.trim();
    if query.is_empty() {
        return Resolved::Nothing;
    }
    // The command word is the leading letters, so `:set nu` and `:s/a/b/`
    // are refused as `:set` and `:s` are (Review Focus 2).
    let word = query.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("");
    if UNSUPPORTED.contains(&word)
        || query.starts_with(|c: char| c.is_ascii_digit() || "%.$'<!".contains(c))
    {
        return Resolved::Refused(format!("Not supported: :{query}"));
    }
    if let Some((head, _)) = query.split_once(char::is_whitespace) {
        return match find(head, |_| true) {
            Some(_) => Resolved::Refused(format!("Not supported: :{query}")),
            None => Resolved::Nothing,
        };
    }
    if let Some(v) = find(query, |v| runs_in(v, scope)) {
        let mut row = row(v);
        // On Manage a window verb closes the screen, and its row says so.
        let closes = row.actions.iter().any(|a| matches!(a, Action::CloseWindow { .. }));
        if closes && crate::keyroute::table::Surfaces::MANAGE.has(scope.surface) {
            row.caption = "Close Manage";
        }
        return Resolved::Pinned(row);
    }
    // A verb the query spells that runs elsewhere, not here.
    match find(query, |_| true) {
        Some(v) => Resolved::Refused(format!("Nothing to :{} here", v.names[0])),
        None => Resolved::Nothing,
    }
}

/// The ex spelling of a fixed action, for keycaps (spec §4.6): `:w` for
/// `SaveRequest`, the shortest name. `None` for an action no verb runs on
/// its own.
pub fn spelling(action: &Action) -> Option<String> {
    VERBS
        .iter()
        .find(|v| matches!(v.run, Run::Act(f) if f() == *action))
        .and_then(|v| v.names.iter().min_by_key(|n| n.len()))
        .map(|n| format!(":{n}"))
}

/// Whether some verb runs this list verb (the reachability test, spec §8.2).
pub fn reaches(verb: Verb) -> bool {
    VERBS.iter().any(|v| matches!(v.run, Run::Verb(w) if w.kind() == verb.kind()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, Verb};
    use crate::keyroute::Surface;

    fn sidebar() -> Scope {
        Scope {
            surface: Surface::Sidebar,
            verbs: vec![
                Verb::Add,
                Verb::Rename,
                Verb::Move,
                Verb::Delete { count: 1 },
                Verb::Yank,
                Verb::Put { above: false, count: 1 },
            ],
        }
    }

    fn pinned(q: &str, scope: &Scope) -> ExRow {
        match resolve(q, scope) {
            Resolved::Pinned(row) => row,
            other => panic!("{q:?}: {other:?}"),
        }
    }

    /// Spec §5.9's table: the file verbs are universal.
    #[test]
    fn the_file_verbs_resolve_everywhere() {
        let close = Action::CloseField;
        for q in ["w", "write", "up", "update", "wa"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::SaveRequest],
                "{q}"
            );
        }
        let window = |write, force| vec![close.clone(), Action::CloseWindow { write, force }];
        assert_eq!(pinned("q", &sidebar()).actions, window(false, false));
        assert_eq!(
            pinned("q!", &sidebar()).actions,
            vec![Action::CloseWindow { write: false, force: true }],
            "no close: it abandons the field"
        );
        for q in ["wq", "x", "xit", "exit"] {
            assert_eq!(pinned(q, &sidebar()).actions, window(true, false), "{q}");
        }
        for q in ["qa", "qall", "quitall"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::GatedQuit],
                "{q}"
            );
        }
        assert_eq!(
            pinned("qa!", &sidebar()).actions,
            vec![Action::ForceQuit],
            "no close: it abandons the field"
        );
        for q in ["wqa", "wqall", "xa", "xall"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::WriteQuit],
                "{q}"
            );
        }
        assert_eq!(pinned("e", &sidebar()).actions, vec![close.clone(), Action::ReloadFromDisk]);
        assert_eq!(
            pinned("e!", &sidebar()).actions,
            vec![close.clone(), Action::DiscardChanges, Action::ReloadFromDisk]
        );
        assert_eq!(pinned("noh", &sidebar()).actions, vec![close.clone(), Action::NoHlSearch]);
        assert_eq!(
            pinned("split", &Scope { surface: Surface::Body, verbs: vec![] }).actions,
            vec![close, Action::CycleSplit]
        );
        assert_eq!(pinned("w", &sidebar()).name, "write", "the pinned row shows the full name");
    }

    /// Plan M5b Deviation 18: Vim's `!` on the write verbs.
    #[test]
    fn the_bang_write_verbs_resolve() {
        let close = Action::CloseField;
        for q in ["w!", "write!", "up!", "update!", "wa!"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::ForceWrite { quit: false }],
                "{q}"
            );
        }
        for q in ["wq!", "x!", "xit!", "exit!"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::CloseWindow { write: true, force: true }],
                "{q}"
            );
        }
        for q in ["wqa!", "wqall!", "xa!", "xall!"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::ForceWrite { quit: true }],
                "{q}"
            );
        }
        assert_eq!(pinned("wr", &sidebar()).name, "write", "a prefix still reaches the plain verb");
        assert_eq!(pinned("upd", &sidebar()).name, "write");
    }

    /// Sweep backlog item 4: on Manage the window verbs' row says what they
    /// do there; the `a` forms and the main screen keep "Quit".
    #[test]
    fn a_window_verb_on_manage_says_it_closes_manage() {
        let manage = Scope {
            surface: Surface::ManageList(crate::components::manage::ManageTab::Spaces),
            verbs: vec![],
        };
        for q in ["q", "q!", "wq", "x", "wq!"] {
            assert_eq!(pinned(q, &manage).caption, "Close Manage", "{q}");
        }
        assert_eq!(pinned("qa", &manage).caption, "Quit");
        assert_eq!(
            pinned("q", &Scope { surface: Surface::Settings, verbs: vec![] }).caption,
            "Close Manage"
        );
        assert_eq!(pinned("q", &sidebar()).caption, "Quit");
    }

    /// Exact beats prefix; the shortest name wins a prefix; ties go to the
    /// earlier row (Deviation 8).
    #[test]
    fn exact_then_shortest_prefix() {
        assert_eq!(pinned("q", &sidebar()).name, "quit", "not qa or q!");
        assert_eq!(pinned("qu", &sidebar()).name, "quit");
        assert_eq!(pinned("sen", &sidebar()).name, "send");
        assert_eq!(pinned("sp", &sidebar()).name, "split");
        assert_eq!(pinned("man", &sidebar()).name, "manage");
        assert_eq!(pinned("ne", &sidebar()).name, "new");
        assert_eq!(pinned("n", &sidebar()).name, "new", "new is listed before nohlsearch");
        assert_eq!(pinned("re", &sidebar()).name, "rename", "rename is listed before reveal");
        assert_eq!(pinned("m", &sidebar()).name, "move", "shorter than manage");
        assert_eq!(pinned("del", &sidebar()).name, "delete");
    }

    #[test]
    fn the_curl_verbs_resolve_by_name_and_prefix() {
        let close = Action::CloseField;
        for q in ["import", "impo", "imp"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::ImportCurl { folder: None }],
                "{q}"
            );
        }
        for q in ["copycurl", "copyc"] {
            assert_eq!(
                pinned(q, &sidebar()).actions,
                vec![close.clone(), Action::CopyAsCurl(None)],
                "{q}"
            );
        }
        assert_eq!(pinned("import", &sidebar()).caption, "Import curl from clipboard");
    }

    /// Vim's own short commands are not prefixes of the curl verbs: `:c`
    /// still meets `:clear`, as before they joined the table.
    #[test]
    fn vims_short_commands_do_not_reach_the_curl_verbs() {
        assert!(
            matches!(resolve("c", &sidebar()), Resolved::Refused(t) if t == "Nothing to :clear here")
        );
        for q in ["co", "cop", "copy", "i", "im"] {
            assert!(matches!(resolve(q, &sidebar()), Resolved::Nothing), "{q}");
        }
        let grid = Scope { surface: Surface::VarGrid, verbs: vec![Verb::ClearValue] };
        assert_eq!(pinned("c", &grid).name, "clear");
    }

    /// Spec §5.9's Out rows: the unsupported spellings and ranges are
    /// refused, and `:s` never pins send.
    #[test]
    fn unsupported_spellings_and_ranges_are_refused() {
        for q in [
            "s",
            "substitute",
            "g",
            "global",
            "v",
            "vglobal",
            "se",
            "set",
            "sor",
            "sort",
            "norm",
            "normal",
            "r",
            "read",
            "!",
            "%s/a/b/",
            "5",
            "1,3d",
            ".w",
            "$",
            "'a",
            "<,'>d",
        ] {
            assert!(
                matches!(resolve(q, &sidebar()), Resolved::Refused(t) if t == format!("Not supported: :{q}")),
                "{q}"
            );
        }
        assert_eq!(
            pinned("sec", &Scope { surface: Surface::VarForm, verbs: vec![Verb::Secret] }).name,
            "secret"
        );
        assert_eq!(
            pinned("gr", &Scope { surface: Surface::VarList, verbs: vec![Verb::AddAlt] }).name,
            "group"
        );
        assert_eq!(
            pinned("va", &Scope { surface: Surface::VarGrid, verbs: vec![Verb::EditValue] }).name,
            "value"
        );
    }

    /// Vim's `:u` is undo, so it never reaches `:up` by prefix and saves
    /// (the code review's first finding); `:red` is refused alongside it.
    #[test]
    fn undo_and_redo_are_refused_not_prefixed() {
        for q in ["u", "undo", "red", "redo"] {
            assert!(
                matches!(resolve(q, &sidebar()), Resolved::Refused(t) if t == format!("Not supported: :{q}")),
                "{q}"
            );
        }
        assert_eq!(pinned("up", &sidebar()).name, "write", ":up is still update");
    }

    /// Spaces around a verb are not part of it; a verb given an argument
    /// is refused rather than left to the fuzzy rows (the code review's
    /// second finding): `:w file` writes elsewhere in Vim, which we do not.
    #[test]
    fn spaces_trim_and_arguments_are_refused() {
        for q in ["w ", " w", " w  "] {
            assert_eq!(pinned(q, &sidebar()).name, "write", "{q:?}");
        }
        for (q, shown) in [
            ("w file", "w file"),
            ("e foo.json ", "e foo.json"),
            ("wr x", "wr x"),
            ("delete 3", "delete 3"),
            ("q!  now", "q!  now"),
        ] {
            assert!(
                matches!(resolve(q, &sidebar()), Resolved::Refused(t) if t == format!("Not supported: :{shown}")),
                "{q:?}"
            );
        }
        assert!(
            matches!(resolve("fields x", &sidebar()), Resolved::Refused(t) if t == "Not supported: :fields x"),
            "a verb from elsewhere too"
        );
        assert!(
            matches!(resolve("ab cd", &sidebar()), Resolved::Nothing),
            "no verb: the fuzzy rows"
        );
        assert!(matches!(resolve("   ", &sidebar()), Resolved::Nothing));
    }

    /// Deviation 9: a verb with no meaning here is refused with its own
    /// text; one that means nothing anywhere resolves to nothing.
    #[test]
    fn a_verb_that_does_not_run_here_is_refused() {
        assert!(
            matches!(resolve("fields", &sidebar()), Resolved::Refused(t) if t == "Nothing to :fields here")
        );
        assert!(
            matches!(resolve("fi", &sidebar()), Resolved::Refused(t) if t == "Nothing to :fields here"),
            "by prefix too"
        );
        assert!(matches!(resolve("zzzz", &sidebar()), Resolved::Nothing));
        assert!(matches!(resolve("", &sidebar()), Resolved::Nothing));
        let list = Scope {
            surface: Surface::VarList,
            verbs: vec![
                Verb::Add,
                Verb::AddAlt,
                Verb::Rename,
                Verb::Delete { count: 1 },
                Verb::Secret,
                Verb::Fields,
            ],
        };
        assert_eq!(
            pinned("fields", &list).actions,
            vec![Action::CloseField, Action::OnSelection(Verb::Fields)]
        );
        assert_eq!(
            pinned(
                "tls",
                &Scope {
                    surface: Surface::ManageList(
                        crate::components::manage::ManageTab::Environments
                    ),
                    verbs: vec![Verb::Tls]
                }
            )
            .actions,
            vec![Action::CloseField, Action::OnSelection(Verb::Tls)]
        );
    }

    /// Spec §4.6: the ex table spells keycaps (`:w save`) with the
    /// shortest name; every list verb has an ex spelling (plan M2
    /// Deviation 9's `VIM_UNTIL_M4` empties).
    #[test]
    fn spellings_and_reach() {
        assert_eq!(spelling(&Action::SaveRequest).as_deref(), Some(":w"));
        assert_eq!(
            spelling(&Action::CloseWindow { write: false, force: false }).as_deref(),
            Some(":q")
        );
        assert_eq!(spelling(&Action::GatedQuit).as_deref(), Some(":qa"));
        assert_eq!(
            spelling(&Action::Quit),
            None,
            "the quit chip spells `:q` itself (`spell::Key::Quit`)"
        );
        assert_eq!(spelling(&Action::CycleSplit).as_deref(), Some(":split"));
        assert_eq!(spelling(&Action::CycleSplitBack).as_deref(), Some(":splitback"));
        assert_eq!(spelling(&Action::Send).as_deref(), Some(":send"));
        assert_eq!(spelling(&Action::Undo), None);
        for v in Verb::ALL {
            let listed = !matches!(v, Verb::Yank | Verb::Put { .. });
            assert_eq!(reaches(v), listed, "{v:?}");
        }
        let mut names: Vec<&str> = VERBS.iter().flat_map(|v| v.names.iter().copied()).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "no spelling twice");
        for v in VERBS {
            assert!(!v.caption.is_empty(), "{:?}", v.names);
        }
    }
}
