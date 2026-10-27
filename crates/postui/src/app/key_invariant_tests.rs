//! Generated invariants over the window and tab keys (plan
//! 2026-10-05-vim-profile-m4a-consolidation). Every start state meets
//! every window and tab input, in both profiles, and each result must keep
//! spec §5.8's rules: a key moves whole or not at all, never strands a
//! live edit behind another pane or tab, and clears the footer's echo.

use super::key_matrix_tests::{Fixture, build, draw, feed, snapshot};
use super::*;
use crate::components::settings::SettingsField;
use crate::config::KeyProfile;
use crate::keyroute::{FieldCaret, FieldId};

/// How a start state is reached from its fixture.
#[derive(Debug, Clone, Copy)]
pub(super) enum Setup {
    /// Keys through `feed`.
    Keys(&'static str),
    /// A Settings edit begun on the OSC 52 limit with this text: `1000`
    /// closes, `abc` is refused (it will not parse).
    SettingsEdit(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Start {
    pub name: &'static str,
    pub fixture: Fixture,
    pub profile: KeyProfile,
    pub setup: Setup,
    /// Whether the setup leaves the caret live in a field.
    pub live: bool,
}

/// Every surface and every screen field a window or tab key can start
/// from, in both profiles. Query boxes and dialog fields are left out:
/// no window or tab row applies there (plan M4a Deviation 2).
pub(super) fn starts() -> Vec<Start> {
    use Fixture::*;
    use KeyProfile::{Arrows, Vim};
    use Setup::{Keys, SettingsEdit};
    let s = |name, fixture, profile, setup, live| Start { name, fixture, profile, setup, live };
    let mut out = Vec::new();
    for profile in [Vim, Arrows] {
        for (name, fixture) in [
            ("sidebar", SidebarThree),
            ("table", TableThreeHeaders),
            ("url line", UrlLine),
            ("method", Method),
            ("response", ResponseLines),
            ("env list", ManageEnvs),
            ("env detail", ManageEnvDetail),
            ("spaces", ManageSpaces),
            ("settings row", Settings),
            ("var list", VarList),
            ("var form", VarForm),
            ("var grid", VarGrid),
            ("sidebar folder", SidebarFolder),
            ("response wide", ResponseWide),
            ("settings files", SettingsFiles),
        ] {
            out.push(s(name, fixture, profile, Keys(""), false));
        }
        // Esc from the body blurs the editor in both profiles: no stop is
        // selected (`SubFocus::None`).
        out.push(s("blurred", Body, profile, Keys("<Esc>"), false));
        out.push(s("body", Body, profile, Keys(""), true));
        out.push(s("settings edit", Settings, profile, SettingsEdit("1000"), true));
        out.push(s("settings refused", Settings, profile, SettingsEdit("abc"), true));
        out.push(s("url open", UrlLine, profile, Keys("<CR>"), true));
    }
    out.push(s("cell open", TableThreeHeaders, Vim, Keys("i<Esc>"), true));
    out.push(s("var form open", VarForm, Vim, Keys("i<Esc>"), true));
    out.push(s("var grid open", VarGrid, Vim, Keys("i<Esc>"), true));
    out.push(s("cell open", TableThreeHeaders, Arrows, Keys("<CR>"), true));
    out.push(s("var form open", VarForm, Arrows, Keys("<CR>x"), true));
    out.push(s("var grid open", VarGrid, Arrows, Keys("<CR>"), true));
    out.push(s("search open", ResponseLines, Arrows, Keys("/x"), true));
    out
}

/// The app at `start`, checked: the caret is where the start says, and no
/// edit is stranded before the input runs.
pub(super) fn begin(start: &Start) -> App {
    let mut app = build(start.fixture);
    app.use_profile(start.profile);
    draw(&mut app);
    match start.setup {
        Setup::Keys(keys) => feed(&mut app, keys),
        Setup::SettingsEdit(text) => {
            app.settings.begin_edit(SettingsField::Osc52Limit, text);
            draw(&mut app);
        }
    }
    assert_eq!(
        app.field_caret().is_live(),
        start.live,
        "{} {:?}: the setup left the caret at {:?}",
        start.name,
        start.profile,
        app.field_caret()
    );
    assert!(
        stranded(&app).is_empty(),
        "{} {:?}: the setup strands {:?}",
        start.name,
        start.profile,
        stranded(&app)
    );
    app
}

/// One window or tab input.
#[derive(Debug, Clone)]
pub(super) enum Input {
    Keys(&'static str),
    Event(KeyEvent),
    /// An action a `keys.toml` binding can dispatch in either profile.
    Act(Action),
}

pub(super) fn inputs(profile: KeyProfile) -> Vec<Input> {
    let mut out: Vec<Input> = match profile {
        KeyProfile::Vim => [
            "gt",
            "gT",
            "1gt",
            "2gt",
            "3gt",
            "4gt",
            "5gt",
            "9gt",
            "3gT",
            "12gT",
            "<C-w>h",
            "<C-w>j",
            "<C-w>k",
            "<C-w>l",
            "<C-w>w",
            "<C-w>W",
            "<C-w>p",
            "<C-w><Left>",
            "<C-w><Down>",
            "<C-w><Up>",
            "<C-w><Right>",
            "<C-w><C-w>",
            "<C-w>x",
        ]
        .into_iter()
        .map(Input::Keys)
        .collect(),
        KeyProfile::Arrows => vec![
            Input::Event(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
            Input::Event(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
        ],
    };
    out.extend([
        Input::Act(Action::CycleTabs(1)),
        Input::Act(Action::CycleTabs(-1)),
        Input::Act(Action::EditorTabCycle(1)),
        Input::Act(Action::EditorTabCycle(-1)),
    ]);
    out
}

/// Runs `input` as the event loop would: the key or action, then the undo
/// capture and a draw.
pub(super) fn apply(app: &mut App, input: &Input) {
    match input {
        Input::Keys(keys) => feed(app, keys),
        Input::Event(ev) => {
            app.handle_key(*ev);
            app.capture_undo();
            draw(app);
        }
        Input::Act(action) => {
            app.update(action.clone());
            app.capture_undo();
            draw(app);
        }
    }
}

/// Every edit live anywhere, named as the field it belongs to: what a key
/// could strand. The body is left out: it is a pane's content, always
/// there, not an edit that opens and closes.
pub(super) fn live_edits(app: &App) -> Vec<FieldId> {
    use crate::components::table_editor::Col;
    let mut out = Vec::new();
    if app.editor.url_open() {
        out.push(FieldId::Url);
    }
    if let Some(e) = &app.editor.table.editing {
        out.push(FieldId::TableCell { row: e.row, col: usize::from(e.col == Col::Value) });
    }
    if let Some(e) = &app.varmanager.form.editing {
        out.push(FieldId::VmForm(e.field));
    }
    if let Some(e) = &app.varmanager.grid.editing {
        out.push(FieldId::VmGrid { row: e.row, col: e.col });
    }
    if let Some(f) = app.settings.editing {
        out.push(FieldId::Settings(f));
    }
    if app.session.response.jq_focused() {
        out.push(FieldId::Jq);
    } else if app.session.response.field_open() {
        out.push(FieldId::Search);
    }
    out
}

/// The live edits the caret is not in: stranded behind another pane or tab.
pub(super) fn stranded(app: &App) -> Vec<FieldId> {
    let caret = match app.field_caret() {
        FieldCaret::Live(id) => Some(id),
        FieldCaret::Parked(_) | FieldCaret::None => None,
    };
    live_edits(app).into_iter().filter(|id| Some(*id) != caret).collect()
}

/// Where the keyboard is, coarsely: what a window or tab key moves.
pub(super) fn place(app: &App) -> String {
    format!(
        "{:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        app.screen,
        app.focus,
        app.manage.tab,
        app.manage.list.focus,
        app.varmanager.focus,
        app.editor.active_tab,
        app.session.response.view().map(|v| v.mode)
    )
}

fn check(start: &Start, input: &Input) -> Result<(), String> {
    let mut app = begin(start);
    let (place0, edits0, caret0) = (place(&app), live_edits(&app), app.field_caret());
    apply(&mut app, input);
    let (place1, edits1, caret1) = (place(&app), live_edits(&app), app.field_caret());
    if place1 == place0 && (edits1 != edits0 || caret1 != caret0) {
        return Err(format!(
            "nothing moved, but the field changed: {edits0:?} {caret0:?} -> {edits1:?} {caret1:?}"
        ));
    }
    let hidden = stranded(&app);
    if !hidden.is_empty() {
        return Err(format!("stranded {hidden:?} (moved {place0} -> {place1})"));
    }
    if !app.key_echo().is_empty() {
        return Err(format!("the echo kept {:?}", app.key_echo()));
    }
    Ok(())
}

/// Spec §5.8: every window and tab input, from every start, moves whole or
/// not at all, strands no live edit, and clears the echo.
#[test]
fn window_and_tab_keys_move_whole_or_not_at_all() {
    let mut failures = Vec::new();
    let mut ran = 0;
    for start in starts() {
        for input in inputs(start.profile) {
            ran += 1;
            if let Err(m) = check(&start, &input) {
                failures.push(format!("{} {:?} {:?}: {m}", start.name, start.profile, input));
            }
        }
    }
    assert!(ran > 400, "the generator shrank: {ran} cases");
    assert!(
        failures.is_empty(),
        "{} of {ran} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Plan Decision 2: `screen_field` owns which screen field has the caret,
/// and `field_open` and `open_text_field_mut` agree with it, from every
/// start and after every window and tab input. Modals are out of scope
/// here: every start has none.
#[test]
fn field_open_and_the_open_text_field_agree_with_screen_field() {
    let agree = |app: &mut App, at: &str| {
        assert!(app.modals.is_empty(), "{at}: no start opens a modal");
        let id = app.screen_field();
        assert_eq!(
            app.field_open(),
            id.is_some_and(|id| id != FieldId::Body),
            "{at}: field_open vs {id:?}"
        );
        // Every screen field but the body is a `LineInput`.
        let has_input = id.is_some_and(|id| id != FieldId::Body);
        assert_eq!(
            app.open_text_field_mut().is_some(),
            has_input,
            "{at}: open_text_field_mut vs {id:?}"
        );
    };
    for start in starts() {
        let mut app = begin(&start);
        agree(&mut app, &format!("{} {:?}", start.name, start.profile));
        for input in inputs(start.profile) {
            let mut app = begin(&start);
            apply(&mut app, &input);
            agree(&mut app, &format!("{} {:?} after {input:?}", start.name, start.profile));
        }
    }
}

/// Everything a key on a surface (no field live) could change, short of
/// the files on disk: where the keyboard is, every list's cursor and
/// aim, the response's view, the table's column and the Settings values.
fn observed(app: &App) -> String {
    format!(
        "{} | {:?} {:?} {:?} {:?} {:?} {} {:?} {:?} {} {:?} {:?} {} {} {:?} {:?} {:?} {}",
        place(app),
        app.sidebar.selected_key(),
        app.sidebar.rows.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>(),
        app.editor.sub_focus,
        app.editor.table.selected(),
        app.editor.table.col,
        app.manage.list.cursor,
        app.manage.list.detail_row,
        app.manage.list.detail_col,
        app.varmanager.left_cursor,
        app.varmanager.grid.cursor,
        app.varmanager.form_cursor,
        app.settings.cursor,
        app.settings.file_button,
        app.session.response.view().map(|v| (v.cursor, v.scroll, v.h_scroll)),
        app.ui_settings,
        app.field_caret(),
        app.modals.is_empty()
    )
}

/// The response's starts, where shift+↑/↓ are named (see below).
fn on_the_response(start: &Start) -> bool {
    matches!(start.fixture, Fixture::ResponseLines | Fixture::ResponseWide)
}

/// Plan Decision 4: on every surface (no field live), in both profiles, a
/// chord that no keymap binds and no component names does nothing: it no
/// longer acts as the bare key. alt+↑/↓ are named on purpose (they move a
/// row in the sidebar and the Manage lists), and so are ctrl+Home/End
/// (first/last row on a list, document jumps on the response; the user's
/// ruling 2026-10-07). The response also names shift+↑/↓ (extend a line
/// selection) on purpose, spec'd by its own tests, so those are exempt
/// there only.
#[test]
fn an_unbound_chord_on_a_list_does_nothing() {
    use KeyCode::*;
    let named = [
        (Up, KeyModifiers::ALT),
        (Down, KeyModifiers::ALT),
        (Home, KeyModifiers::CONTROL),
        (End, KeyModifiers::CONTROL),
    ];
    let response_named = [(Up, KeyModifiers::SHIFT), (Down, KeyModifiers::SHIFT)];
    let codes = [Up, Down, Left, Right, Home, End, PageUp, PageDown, Enter];
    let mods = [
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT | KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ];
    let mut failures = Vec::new();
    for start in starts().into_iter().filter(|s| !s.live) {
        for code in codes {
            for m in mods {
                if named.contains(&(code, m))
                    || (on_the_response(&start) && response_named.contains(&(code, m)))
                {
                    continue;
                }
                let ev = KeyEvent::new(code, m);
                let mut app = begin(&start);
                if app.keymap.lookup(&crate::keys::KeyCombo::from_event(&ev)).is_some() {
                    continue;
                }
                let before = (observed(&app), app.project().is_some().then(|| snapshot(&app)));
                app.handle_key(ev);
                app.capture_undo();
                draw(&mut app);
                let after = (observed(&app), app.project().is_some().then(|| snapshot(&app)));
                if after != before {
                    failures.push(format!(
                        "{} {:?} {m:?}+{code:?}:\n  before {}\n  after  {}",
                        start.name, start.profile, before.0, after.0
                    ));
                }
            }
        }
    }
    // One line per case, then the first in full: the states are long.
    let heads: Vec<&str> = failures.iter().filter_map(|f| f.lines().next()).collect();
    assert!(
        failures.is_empty(),
        "{} chord(s) acted as the bare key:\n{}\n\nThe first in full:\n{}",
        failures.len(),
        heads.join("\n"),
        failures.first().map_or("", String::as_str)
    );
}

/// Spec §4.4 (plan M4b Deviation 19): vim's keymap gives up ctrl+s, alt+w
/// and shift+alt+w; with no component naming them, they do nothing in vim
/// on every start (the disk included), while arrows keeps them.
#[test]
fn vim_gives_up_ctrl_s_and_the_split_chords_without_a_leak() {
    let chords = [
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
        KeyEvent::new(KeyCode::Char('W'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT | KeyModifiers::SHIFT),
    ];
    let mut failures = Vec::new();
    let mut ran = 0;
    for start in starts().into_iter().filter(|s| s.profile == KeyProfile::Vim) {
        for ev in chords {
            ran += 1;
            let mut app = begin(&start);
            assert!(
                app.keymap.lookup(&crate::keys::KeyCombo::from_event(&ev)).is_none(),
                "{}: vim binds {ev:?}",
                start.name
            );
            let before = (
                observed(&app),
                app.project().is_some().then(|| snapshot(&app)),
                app.split_state().stop(),
            );
            app.handle_key(ev);
            app.capture_undo();
            draw(&mut app);
            let after = (
                observed(&app),
                app.project().is_some().then(|| snapshot(&app)),
                app.split_state().stop(),
            );
            if after != before {
                failures.push(format!(
                    "{} {:?}:\n  before {}\n  after  {}",
                    start.name, ev, before.0, after.0
                ));
            }
        }
    }
    assert!(ran >= 40, "the generator shrank: {ran}");
    assert!(
        failures.is_empty(),
        "{} chord(s) acted in vim:\n{}",
        failures.len(),
        failures.join("\n")
    );
    let arrows = crate::keys::Keymap::default_bindings_for(KeyProfile::Arrows);
    assert_eq!(
        arrows.lookup(&crate::keys::KeyCombo::from_event(&chords[0])),
        Some(Action::SaveRequest)
    );
    assert_eq!(
        arrows.lookup(&crate::keys::KeyCombo::from_event(&chords[1])),
        Some(Action::CycleSplit)
    );
}

/// Review Focus 3: the router replays vim's motion letters as bare named
/// keys, so the guard leaves them working.
#[test]
fn vim_motions_still_move_the_sidebar_after_the_guard() {
    let start = Start {
        name: "sidebar",
        fixture: Fixture::SidebarThree,
        profile: KeyProfile::Vim,
        setup: Setup::Keys(""),
        live: false,
    };
    let mut app = begin(&start);
    feed(&mut app, "G");
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"));
    feed(&mut app, "k");
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/beta"));
    feed(&mut app, "gg");
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/alpha"));
}

/// The user's ruling 2026-10-07: on every list, ctrl+Home/End are a second
/// spelling of Home/End (first and last row), as the response's document
/// jumps are; macOS's cmd+↑/↓ fold to them. The response is left out:
/// there bare Home/End scroll sideways, and the chords are spec'd by its
/// own tests.
#[test]
fn ctrl_home_and_end_are_home_and_end_on_every_list() {
    let mut moved = Vec::new();
    let mut failures = Vec::new();
    for start in starts().into_iter().filter(|s| !s.live && !on_the_response(s)) {
        for code in [KeyCode::Home, KeyCode::End] {
            let run = |m| {
                let mut app = begin(&start);
                let before = observed(&app);
                app.handle_key(KeyEvent::new(code, m));
                app.capture_undo();
                draw(&mut app);
                (before, observed(&app))
            };
            let (before, bare) = run(KeyModifiers::NONE);
            let (_, chord) = run(KeyModifiers::CONTROL);
            if bare != before {
                moved.push((start.name, start.profile));
            }
            if chord != bare {
                failures.push(format!(
                    "{} {:?} ctrl+{code:?}:\n  bare  {bare}\n  chord {chord}",
                    start.name, start.profile
                ));
            }
        }
    }
    let heads: Vec<&str> = failures.iter().filter_map(|f| f.lines().next()).collect();
    assert!(
        failures.is_empty(),
        "{} chord(s) differ from the bare key:\n{}\n\nThe first in full:\n{}",
        failures.len(),
        heads.join("\n"),
        failures.first().map_or("", String::as_str)
    );
    // Each list's Home or End moves it, so the check above can fail there.
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        for name in [
            "sidebar",
            "sidebar folder",
            "table",
            "env list",
            "env detail",
            "spaces",
            "settings row",
            "settings files",
            "var list",
            "var form",
            "var grid",
        ] {
            assert!(moved.contains(&(name, profile)), "Home or End moves {name} {profile:?}");
        }
    }
}

/// Final review I-1: on macOS, cmd+↑/↓ (SUPER, from a kitty-protocol
/// terminal) fold to ctrl+Home/End at the top of the app's key path
/// (`keys::normalize_super_keys`), so they jump to the sidebar's first and
/// last row.
#[test]
fn cmd_up_and_down_jump_to_the_sidebars_first_and_last_row() {
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let start = Start {
            name: "sidebar",
            fixture: Fixture::SidebarThree,
            profile,
            setup: Setup::Keys(""),
            live: false,
        };
        let mut app = begin(&start);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER));
        draw(&mut app);
        assert_eq!(
            app.sidebar.selected_slug().as_deref(),
            Some("main/alpha"),
            "{profile:?}: cmd+↑"
        );
        let mut app = begin(&start);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER));
        draw(&mut app);
        assert_eq!(
            app.sidebar.selected_slug().as_deref(),
            Some("main/gamma"),
            "{profile:?}: cmd+↓"
        );
    }
}

/// Review Focus 4: the paging chords name CONTROL exactly and keep
/// working, on every list that pages, in both profiles. Each chord runs
/// from the edge it pages away from (the first row for ctrl+d/f, the last
/// for ctrl+u/b), so every one of them has somewhere to go. Left out, as
/// they cannot page: the Settings rows and the Variables form name no
/// paging keys, and the Manage detail pane and the Variables grid hold them
/// inert on purpose.
#[test]
fn paging_chords_survive_the_guard() {
    use Fixture::*;
    let pages = |f: Fixture| {
        matches!(
            f,
            SidebarThree | TableThreeHeaders | ResponseWide | ManageEnvs | ManageSpaces | VarList
        )
    };
    let cursor = |app: &App, f: Fixture| match f {
        SidebarThree => app.sidebar.selected,
        // The stop's place in the walk: rows, `+ Add`, the section.
        TableThreeHeaders => app.editor.table.selected().map(|s| s.ordinal(app.editor.table_len())),
        ResponseWide => app.session.response.view().map(|v| v.cursor),
        ManageEnvs | ManageSpaces => Some(app.manage.list.cursor),
        VarList => Some(app.varmanager.left_cursor),
        _ => unreachable!("{f:?} does not page"),
    };
    let mut ran = 0;
    for start in starts().into_iter().filter(|s| !s.live && pages(s.fixture)) {
        for (letter, down) in [('d', true), ('f', true), ('u', false), ('b', false)] {
            ran += 1;
            let mut app = begin(&start);
            // The edge: the response's bare Home/End scroll sideways, so
            // its document jumps are the chords.
            let edge = if down { KeyCode::Home } else { KeyCode::End };
            let mods =
                if on_the_response(&start) { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
            app.handle_key(KeyEvent::new(edge, mods));
            draw(&mut app);
            let at = cursor(&app, start.fixture).expect("a row is selected");
            app.handle_key(KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL));
            draw(&mut app);
            let now = cursor(&app, start.fixture).expect("a row is selected");
            assert!(
                if down { now > at } else { now < at },
                "{} {:?}: ctrl+{letter} from {at} left the cursor at {now}",
                start.name,
                start.profile
            );
        }
    }
    assert_eq!(ran, 2 * 6 * 4, "every paging list, both profiles, four chords");
}

/// An editor stop the footer's send-slot and tab chips are checked on.
#[derive(Debug, Clone, Copy)]
enum EditorStop {
    Blurred,
    Method,
    UrlSelected,
    UrlOpen,
    TableRow,
    /// Vim's table row with a count half-typed; arrows' as `TableRow`.
    TablePending,
    TableAdd,
    CellOpen,
    Body,
    /// Vim's body in Insert; arrows' body as `Body`.
    BodyInsert,
    /// The URL line selected on the Body tab.
    BodyTabUrl,
}

/// `stop` in `profile`, with the open request's send in flight.
fn sending_at(profile: KeyProfile, stop: EditorStop) -> App {
    use EditorStop as S;
    let fixture = match stop {
        S::Body | S::BodyInsert | S::BodyTabUrl => Fixture::Body,
        S::Method => Fixture::Method,
        S::UrlSelected | S::UrlOpen => Fixture::UrlLine,
        _ => Fixture::TableThreeHeaders,
    };
    let mut app = build(fixture);
    app.use_profile(profile);
    app.editor.url = LineInput::new("http://127.0.0.1:9");
    draw(&mut app);
    let vim = profile == KeyProfile::Vim;
    match stop {
        S::Blurred => app.editor.sub_focus = SubFocus::None,
        S::TableAdd => app.editor.table.set_selected(Some(Stop::Add)),
        S::BodyTabUrl => app.editor.sub_focus = SubFocus::Url,
        S::UrlOpen => feed(&mut app, "<CR>"),
        S::CellOpen => feed(&mut app, if vim { "i" } else { "<CR>" }),
        S::BodyInsert if vim => feed(&mut app, "i"),
        _ => {}
    }
    app.update(Action::ForceSend);
    draw(&mut app);
    if matches!(stop, S::TablePending) && vim {
        feed(&mut app, "2");
        assert_eq!(app.key_echo(), "2", "the count is pending");
    }
    assert!(!app.session.in_flight.is_empty(), "{stop:?}: the send is in flight");
    app
}

/// The footer's `esc cancel` and tab chips are there exactly where their
/// keys do what they say (#193, the user: "probably don't put esc in the
/// footer when in another input", and for the tab chip "same as esc"): on
/// every editor stop, in both profiles, with a send in flight, the chip
/// shows only where the key, pressed, cancels the send or switches the
/// tab, and `esc cancel` shows wherever Esc cancels. In arrows the tab chip
/// shows wherever alt+→ switches the tab too; vim's is spelled from its
/// field's rows, which this does not pin.
#[tokio::test]
async fn the_editors_cancel_and_tab_chips_show_where_their_keys_work() {
    use EditorStop as S;
    let stops = [
        S::Blurred,
        S::Method,
        S::UrlSelected,
        S::UrlOpen,
        S::TableRow,
        S::TablePending,
        S::TableAdd,
        S::CellOpen,
        S::Body,
        S::BodyInsert,
        S::BodyTabUrl,
    ];
    let mut wrong = Vec::new();
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        for stop in stops {
            let app = sending_at(profile, stop);
            let chips = crate::keyroute::spell::resolve(
                crate::ui::footer_chips_of(&app),
                app.spell_ctx(),
                &app.keymap,
            );
            let shows = |label: &str| chips.iter().any(|(_, l, _)| l == label);
            // The keys pressed below are the ones the chips name.
            let tab_key = if profile == KeyProfile::Vim { "gT/gt" } else { "alt+←→" };
            for (k, l, _) in &chips {
                let named = match l.as_str() {
                    "cancel" => "esc",
                    "tabs" => tab_key,
                    _ => continue,
                };
                assert_eq!(*k, crate::keys::display_keycap(named), "{profile:?} {stop:?} {l}");
            }
            let mut esc = sending_at(profile, stop);
            feed(&mut esc, "<Esc>");
            let cancels = esc.session.in_flight.is_empty();
            if shows("cancel") != cancels {
                wrong.push(format!(
                    "{profile:?} {stop:?}: esc cancel shown {}, Esc cancels {cancels}",
                    shows("cancel")
                ));
            }
            // The tab chip's key: arrows' alt+→, vim's `gt`.
            let mut tab = sending_at(profile, stop);
            let before = tab.editor.active_tab;
            match profile {
                KeyProfile::Arrows => {
                    tab.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
                }
                KeyProfile::Vim => feed(&mut tab, "gt"),
            }
            let cycles = tab.editor.active_tab != before;
            let both_ways = profile == KeyProfile::Arrows;
            if shows("tabs") && !cycles || both_ways && cycles && !shows("tabs") {
                wrong.push(format!(
                    "{profile:?} {stop:?}: tabs chip shown {}, its key switches the tab {cycles}",
                    shows("tabs")
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The body takes alt+←/→, not the tab chord: a `keys.toml` that moves the
/// chord onto keys the body leaves alone keeps its chip in the body, and
/// those keys switch the tab there (#193).
#[test]
fn a_rebound_tab_chord_keeps_its_chip_in_the_arrows_body() {
    let mut app = build(Fixture::Body);
    app.keymap.apply_overrides("tab_next = [\"ctrl+n\"]\ntab_prev = [\"ctrl+b\"]\n").unwrap();
    draw(&mut app);
    let chips = crate::keyroute::spell::resolve(
        crate::ui::footer_chips_of(&app),
        app.spell_ctx(),
        &app.keymap,
    );
    assert!(chips.iter().any(|(k, l, _)| k == "^B/^N" && *l == "tabs"), "{chips:?}");
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_ne!(app.editor.active_tab, EditorTab::Body, "the chip's key switched the tab");
}
