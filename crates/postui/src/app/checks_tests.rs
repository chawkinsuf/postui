//! The request's Checks tab and the response's (spec 2026-11-16 checks
//! tabs), through the app, in both key profiles.

use super::key_matrix_tests::{draw, feed, use_persisted_profile};
use super::*;
use crate::components::table_editor::Stop;
use crate::config::KeyProfile;
use crate::keyroute::{FieldCaret, FieldId};
use postui_core::model::{Checks, HttpRequest};

const PROFILES: [KeyProfile; 2] = [KeyProfile::Arrows, KeyProfile::Vim];

/// A request with a status check, a total limit and two jq checks.
const FOUR: &str =
    "url = \"http://x.test/\"\n[checks]\nstatus = 200\nmax_elapsed_ms = 2000\njq = ['.a', '.b']\n";

/// An app with the request `toml` saved and open, the Checks tab up and
/// the caret in its content (on the status field), in `profile`.
fn checks_app(toml: &str, profile: KeyProfile) -> App {
    let mut app = App::new_for_test();
    let req = HttpRequest::from_toml_str(toml).unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/probe", &req).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/probe".into()));
    assert_eq!(app.editor.slug.as_deref(), Some("main/probe"));
    use_persisted_profile(&mut app, profile);
    app.update(Action::EditorTabSelect(EditorTab::Checks.index()));
    app.focus = PaneId::Editor;
    app.editor.focus_content_from_app();
    app.capture_undo();
    draw(&mut app);
    app.toasts = Default::default();
    app
}

fn checks(app: &App) -> Checks {
    app.editor.current_request().checks
}

fn cursor(app: &App) -> Option<Stop> {
    app.editor.table.selected()
}

/// The stop at place `p` of the tab's walk over `n` checks.
fn place(n: usize, p: usize) -> Stop {
    Stop::at(p, 3, n)
}

fn open(profile: KeyProfile) -> &'static str {
    if profile == KeyProfile::Vim { "i" } else { "<CR>" }
}

/// Retypes the open input as `text` and closes it with Enter.
fn retype(app: &mut App, text: &str) {
    app.editor.table.editing.as_mut().expect("an open input").input.set_text(text);
    let close = if app.profile() == KeyProfile::Vim { "<Esc><CR>" } else { "<CR>" };
    feed(app, close);
    assert!(app.editor.table.editing.is_none(), "closed");
}

fn click(app: &mut App, hit: Hit, button: ratatui::crossterm::event::MouseButton) {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    draw(app);
    let r = app.hits.rect_of(&hit).unwrap_or_else(|| panic!("no {hit:?}"));
    // A right press is the whole gesture: its menu opens on it.
    let release = (button == Left).then_some(MouseEventKind::Up(button));
    for kind in [MouseEventKind::Down(button)].into_iter().chain(release) {
        let ev = MouseEvent { kind, column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE };
        app.handle_mouse(ev);
    }
    app.capture_undo();
    draw(app);
}

use ratatui::crossterm::event::MouseButton::{Left, Right};

/// The walk: every move key from every stop, at rest and out of an open
/// field or check, lands where the table's one walk puts it: the fields
/// are places 0 to 2, the checks follow, `+ Add check` is last. ↑ on the
/// status field climbs out to the URL line, as from a table's first row.
#[test]
fn every_move_key_walks_the_fields_the_checks_and_add_as_one() {
    type Rule = fn(usize, usize) -> usize;
    let up: Rule = |p, _| p.saturating_sub(1);
    let down: Rule = |p, last| (p + 1).min(last);
    let rest: &[(&str, Rule)] = &[
        ("<Up>", up),
        ("<Down>", down),
        ("<Home>", |_, _| 0),
        ("<PageUp>", |_, _| 0),
        ("<End>", |_, last| last),
        ("<PageDown>", |_, last| last),
        ("<C-d>", |p, last| (p + last.div_ceil(2)).min(last)),
        ("<C-u>", |p, last| p.saturating_sub(last.div_ceil(2))),
    ];
    let vim_rest: &[(&str, Rule)] = &[
        ("k", up),
        ("j", down),
        ("2j", |p, last| (p + 2).min(last)),
        ("gg", |_, _| 0),
        ("G", |_, last| last),
        ("3G", |_, _| 2),
    ];
    let (n, last) = (2, 5);
    for profile in PROFILES {
        let vim = profile == KeyProfile::Vim;
        let start = |p: usize| {
            let mut app = checks_app(FOUR, profile);
            app.editor.table.set_selected(Some(place(n, p)));
            draw(&mut app);
            app
        };
        let keys = rest.iter().chain(vim_rest.iter().filter(|_| vim));
        for (p, &(keys, rule)) in (0..=last).flat_map(|p| keys.clone().map(move |k| (p, k))) {
            let at = format!("{profile:?} from {p}: {keys}");
            let mut app = start(p);
            feed(&mut app, keys);
            if p == 0 && matches!(keys, "<Up>" | "k") {
                assert_eq!(app.editor.sub_focus, SubFocus::Url, "{at}: climbs out");
                continue;
            }
            assert_eq!(cursor(&app), Some(place(n, rule(p, last))), "{at}");
            assert_eq!(app.editor.sub_focus, SubFocus::Content, "{at}");
        }
        // Out of an open field or check: ↑↓ close it and move one stop.
        for (p, (key, rule)) in
            (0..=last).flat_map(|p| [("<Up>", up), ("<Down>", down)].map(|k| (p, k)))
        {
            let at = format!("{profile:?} open at {p}: {key}");
            let mut app = start(p);
            feed(&mut app, open(profile));
            assert!(app.field_caret().is_live(), "{at}: opened");
            feed(&mut app, if vim { "<Esc>" } else { "" });
            feed(
                &mut app,
                if vim && key == "<Up>" {
                    "k"
                } else if vim {
                    "j"
                } else {
                    key
                },
            );
            // Vim's `k` closes the field, then moves as `k` at rest does.
            if vim && p == 0 && key == "<Up>" {
                assert_eq!(app.editor.sub_focus, SubFocus::Url, "{at}: climbs out");
                continue;
            }
            assert_eq!(cursor(&app), Some(place(n, rule(p, last))), "{at}");
            assert!(!app.field_caret().is_live(), "{at}: closed");
            assert_eq!(checks(&app), checks(&start(p)), "{at}: nothing changed");
        }
    }
}

/// Q1 to Q3: the tab opens unchanged, on its fields and one-column rows,
/// the caret on the status field. Tab in an open field goes on through
/// the fields, then the checks, then `+ Add check`, and out; Shift-Tab
/// comes back the same way.
#[test]
fn tab_walks_the_open_inputs_across_fields_and_checks() {
    let field = |k| FieldCaret::Live(FieldId::CheckField(k));
    let cell = |row| FieldCaret::Live(FieldId::TableCell { row, col: 0 });
    let stops = [field(0), field(1), field(2), cell(0), cell(1), cell(2)];
    let mut app = checks_app(FOUR, KeyProfile::Arrows);
    assert_eq!(app.surface(), crate::keyroute::Surface::Table(EditorTab::Checks));
    assert_eq!(cursor(&app), Some(Stop::Head(0)));
    assert!(!app.editor.is_dirty(), "a request opens unchanged");
    let texts: Vec<String> = (0..3).map(|k| app.editor.check_field_text(k)).collect();
    assert_eq!(texts, ["200", "", "2000"]);
    assert!(app.hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).is_none(), "one column");
    feed(&mut app, "<CR>");
    for (i, stop) in stops.iter().enumerate() {
        assert_eq!(app.field_caret(), *stop, "forward, stop {i}");
        feed(&mut app, "<Tab>");
    }
    assert_eq!((app.field_caret(), cursor(&app)), (FieldCaret::None, Some(Stop::Add)));
    feed(&mut app, "<CR>");
    for (i, stop) in stops.iter().enumerate().rev() {
        assert_eq!(app.field_caret(), *stop, "back, stop {i}");
        feed(&mut app, "<S-Tab>");
    }
    assert_eq!((app.field_caret(), cursor(&app)), (FieldCaret::None, Some(Stop::Head(0))));
    assert!(!app.editor.is_dirty(), "a walk that types nothing changes nothing");
}

/// Q2, Q5, Q7 and the unsaved mark: a field takes what the file can hold
/// and refuses the rest with a warning, keeping its value; each change is
/// one undo step; an edit taken back is no change; Discard clears it.
#[test]
fn a_field_takes_a_value_or_refuses_it_and_undoes_in_one_step() {
    for profile in PROFILES {
        let undo = if profile == KeyProfile::Vim { "u" } else { "<C-z>" };
        let mut app = checks_app(FOUR, profile);
        feed(&mut app, open(profile));
        assert_eq!(app.field_caret(), FieldCaret::Live(FieldId::CheckField(0)), "{profile:?}");
        retype(&mut app, "200, 201");
        assert_eq!(checks(&app).status, [200, 201], "{profile:?}");
        assert!(app.editor.is_dirty(), "{profile:?}");
        feed(&mut app, open(profile));
        retype(&mut app, "99");
        assert_eq!(checks(&app).status, [200, 201], "{profile:?}: refused, the old value stays");
        let said = app.toasts.last_message().unwrap_or_default().to_string();
        assert!(said.contains("100 to 599") && said.contains("kept '200, 201'"), "{said}");
        feed(&mut app, undo);
        assert_eq!(checks(&app).status, [200], "{profile:?}: one step back");
        assert!(!app.editor.is_dirty(), "{profile:?}: an edit taken back is no change");
        assert_eq!(cursor(&app), Some(Stop::Head(0)), "{profile:?}: the cursor stays");
        app.update(Action::Redo);
        assert_eq!(checks(&app).status, [200, 201], "{profile:?}: redo");
        feed(&mut app, undo);
        // The limits: empty is no limit, 0 and text are refused.
        app.editor.table.set_selected(Some(Stop::Head(2)));
        for (typed, want) in
            [("0", Some(2000)), ("2 s", Some(2000)), ("", None), ("750", Some(750))]
        {
            feed(&mut app, open(profile));
            retype(&mut app, typed);
            assert_eq!(checks(&app).max_elapsed_ms, want, "{profile:?}: {typed:?}");
        }
        app.update(Action::DiscardChanges);
        assert_eq!(checks(&app).max_elapsed_ms, Some(2000), "{profile:?}: Discard");
        assert!(!app.editor.is_dirty(), "{profile:?}");
    }
}

/// Q3, Q4, Q7: add, edit, copy, delete and put on a check, each one undo
/// step, worded for a check; on a field copy and delete do nothing, add
/// and put act as on `+ Add check`.
#[test]
fn a_check_answers_the_keys_a_headers_row_does() {
    for profile in PROFILES {
        let vim = profile == KeyProfile::Vim;
        let (copy, delete, undo) = if vim { ("yy", "dd", "u") } else { ("y", "d", "<C-z>") };
        let jq = |app: &App| checks(app).jq;
        let mut app = checks_app(FOUR, profile);
        // On a field: copy and delete do nothing.
        feed(&mut app, copy);
        feed(&mut app, delete);
        assert_eq!(
            (checks(&app), app.toasts.last_message()),
            (checks(&checks_app(FOUR, profile)), None)
        );
        // Add, from a field: `+ Add check` opens; a new check goes last.
        feed(&mut app, "a");
        assert_eq!(app.field_caret(), FieldCaret::Live(FieldId::TableCell { row: 2, col: 0 }));
        retype(&mut app, ".items | length == 4");
        assert_eq!(jq(&app), [".a", ".b", ".items | length == 4"], "{profile:?}");
        feed(&mut app, undo);
        assert_eq!(jq(&app), [".a", ".b"], "{profile:?}: the add is one step");
        // Edit a check in place.
        app.editor.table.set_selected(Some(Stop::Row(0)));
        feed(&mut app, open(profile));
        retype(&mut app, ".a == 1");
        assert_eq!(jq(&app), [".a == 1", ".b"], "{profile:?}");
        // Copy: the check's text.
        feed(&mut app, copy);
        let copied = app.resolve_copy(&CopyTarget::TableRow(0)).unwrap();
        assert_eq!(copied, (".a == 1".to_string(), "Copied check".to_string()), "{profile:?}");
        // Delete never asks; undo brings it back under the cursor.
        feed(&mut app, delete);
        assert_eq!(jq(&app), [".b"], "{profile:?}");
        assert!(app.modals.is_empty(), "{profile:?}: a delete never asks");
        let said = app.toasts.last_message().unwrap_or_default().to_string();
        assert!(said.starts_with("Deleted check \".a == 1\""), "{profile:?}: {said}");
        feed(&mut app, undo);
        assert_eq!(
            (jq(&app), cursor(&app)),
            (vec![".a == 1".into(), ".b".into()], Some(Stop::Row(0)))
        );
        // Put: a check still in the list adds none; one that was deleted
        // goes back in, from a check or from a field.
        feed(&mut app, if vim { "yyp" } else { "yp" });
        assert_eq!(jq(&app).len(), 2, "{profile:?}: no repeat");
        let said = app.toasts.last_message().unwrap_or_default().to_string();
        assert_eq!(said, "'.a == 1' already exists \u{2014} no check added", "{profile:?}");
        feed(&mut app, delete);
        app.editor.table.set_selected(Some(Stop::Head(1)));
        feed(&mut app, "p");
        assert_eq!(jq(&app), [".b", ".a == 1"], "{profile:?}: put from a field goes last");
    }
}

/// The table's own refusals, and a counted delete, say "check". A check
/// typed with whitespace around it is the check without it.
#[test]
fn every_message_on_the_tab_says_check() {
    let mut app = checks_app(FOUR, KeyProfile::Vim);
    let last = |app: &App| app.toasts.last_message().unwrap_or_default().to_string();
    feed(&mut app, "a");
    retype(&mut app, ".b ");
    assert_eq!(last(&app), "'.b' already exists \u{2014} no check added");
    app.editor.table.set_selected(Some(Stop::Row(0)));
    feed(&mut app, "i");
    retype(&mut app, "  ");
    assert_eq!(last(&app), "a check can't be empty — kept '.a'");
    feed(&mut app, "i");
    retype(&mut app, " .b");
    assert_eq!(last(&app), "'.b' already exists — the two checks are now one");
    feed(&mut app, "u");
    feed(&mut app, "2dd");
    assert!(last(&app).starts_with("Deleted 2 checks"), "{}", last(&app));
    // The footer's chips and the pills' hints, on a check.
    feed(&mut app, "u");
    let labels: Vec<String> = crate::ui::footer_chips_of(&app).into_iter().map(|c| c.1).collect();
    assert_eq!(labels[..3], ["add check", "copy", "delete"], "{labels:?}");
    assert!(!labels.iter().any(|l| l == "vars" || l == "disable"), "{labels:?}");
    let hint = |app: &App, hit: Hit| crate::hint::hint_for(&hit, &app.keymap, &app.hint_ctx(&hit));
    assert_eq!(hint(&app, Hit::TableCopy(0)).as_deref(), Some("Copy this check"));
    assert_eq!(hint(&app, Hit::TableDelete(0)).as_deref(), Some("Delete this check"));
    let add = || Hit::FooterChip(Action::TableAddRow);
    assert_eq!(hint(&app, add()).as_deref(), Some("Add a check"));
    // The other tabs' add, copy and delete hints are worded as before.
    for tab in [EditorTab::Params, EditorTab::Headers, EditorTab::Vars] {
        app.update(Action::EditorTabSelect(tab.index()));
        let copy = Hit::FooterChip(Action::CopyToClipboard(CopyTarget::TableRow(0)));
        let said = [add(), copy, Hit::TableDelete(0)].map(|hit| hint(&app, hit).unwrap());
        let rows = ["Add a row to the active Params/Headers/Vars table", "Copy this row's value"];
        assert_eq!(said, [rows[0], rows[1], "Delete this row"], "{tab:?}");
    }
    // A yanked check put where it can't go: another tab quotes it on one
    // short line, and the sidebar says the register holds a check.
    let key = format!(".a\n| {}", ". == 1 or ".repeat(20));
    let entry = postui_core::model::Entry { value: String::new(), enabled: true };
    app.yank = crate::yank::Register::Row { tab: EditorTab::Checks, key: key.clone(), entry };
    app.update(Action::EditorTabSelect(EditorTab::Headers.index()));
    app.update(Action::PutTableRow { at: 0, count: 1 });
    let quoted = postui_core::checks::brief(&key);
    assert_eq!(last(&app), format!("{quoted} was yanked from another tab"));
    app.update(Action::PutRequest { anchor: None, above: false, count: 1 });
    assert_eq!(last(&app), "nothing to put here: the register holds a check");
    // A refused field value is quoted short too.
    app.update(Action::EditorTabSelect(EditorTab::Checks.index()));
    app.editor.focus_content_from_app();
    feed(&mut app, "i");
    retype(&mut app, &"9".repeat(200));
    assert!(last(&app).contains("100 to 599, not 999"), "{}", last(&app));
    assert!(last(&app).chars().count() < 150, "{}", last(&app));
}

/// A click opens a field or a check in place, committing what was open; a
/// pill acts on its check; a right click puts the cursor on the part and
/// offers a check its delete alone.
#[test]
fn clicks_open_commit_and_act_as_on_a_headers_row() {
    let mut app = checks_app(FOUR, KeyProfile::Arrows);
    click(&mut app, Hit::CheckField(1), Left);
    assert_eq!(app.field_caret(), FieldCaret::Live(FieldId::CheckField(1)));
    app.editor.table.editing.as_mut().unwrap().input.set_text("300");
    // A click on a check commits the field and opens the check.
    click(&mut app, Hit::TableCell { row: 1, col: 0 }, Left);
    assert_eq!(checks(&app).max_ttfb_ms, Some(300));
    assert_eq!(app.field_caret(), FieldCaret::Live(FieldId::TableCell { row: 1, col: 0 }));
    // A click on a field commits the check; a refused field value toasts.
    app.editor.table.editing.as_mut().unwrap().input.set_text(".b == 2");
    click(&mut app, Hit::CheckField(0), Left);
    assert_eq!(checks(&app).jq, [".a", ".b == 2"]);
    app.editor.table.editing.as_mut().unwrap().input.set_text("abc");
    click(&mut app, Hit::TableCell { row: 0, col: 0 }, Left);
    assert_eq!(checks(&app).status, [200], "refused");
    assert!(app.toasts.last_message().unwrap_or_default().contains("100 to 599"));
    // A click whose own commit changes what is under the pointer: the open
    // check merges into the other, and its delete pill takes the one row
    // the commit left it on, as a Headers row's does.
    app.editor.table.editing.as_mut().unwrap().input.set_text(".b == 2");
    click(&mut app, Hit::TableDelete(0), Left);
    assert!(checks(&app).jq.is_empty());
    assert_eq!(cursor(&app), Some(Stop::Add));
    feed(&mut app, "<C-z><C-z>");
    assert_eq!(checks(&app).jq, [".a", ".b == 2"], "the delete and the merge are a step each");
    // A row click moves the cursor; a pill moves none.
    app.editor.table.set_selected(Some(Stop::Head(2)));
    click(&mut app, Hit::TableRow(1), Left);
    assert_eq!(cursor(&app), Some(Stop::Row(1)));
    app.hovered = Some(Hit::TableCell { row: 0, col: 0 });
    click(&mut app, Hit::TableCopy(0), Left);
    assert_eq!(app.toasts.last_message(), Some("Copied check"));
    assert_eq!(cursor(&app), Some(Stop::Row(1)), "a pill moves no cursor");
    // A right click: the cursor goes on the part; a check's menu is Delete.
    click(&mut app, Hit::CheckField(2), Right);
    assert_eq!((cursor(&app), app.modals.is_empty()), (Some(Stop::Head(2)), true));
    click(&mut app, Hit::TableCell { row: 0, col: 0 }, Right);
    assert_eq!(cursor(&app), Some(Stop::Row(0)));
    let Some(Modal::Dropdown(menu)) = app.modals.top() else { panic!("the row menu") };
    let items: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(items, ["Delete check"]);
}

/// Q4: a paste into a field or a check lands on one line, by the
/// terminal's paste and by ctrl+v, in both profiles.
#[test]
fn a_paste_into_a_field_or_a_check_is_one_line() {
    for profile in PROFILES {
        let mut app = checks_app("url = \"http://x.test/\"\n", profile);
        feed(&mut app, open(profile));
        assert!(app.handle_paste("201,\n204"), "{profile:?}");
        feed(&mut app, if profile == KeyProfile::Vim { "<Esc><CR>" } else { "<CR>" });
        assert_eq!(checks(&app).status, [201, 204], "{profile:?}");
        feed(&mut app, "a");
        app.clipboard.set_read_for_test(".items\n  | length == 4\n");
        app.update(Action::Paste);
        feed(&mut app, if profile == KeyProfile::Vim { "<Esc><CR>" } else { "<CR>" });
        let jq = checks(&app).jq;
        assert_eq!(jq.len(), 1, "{profile:?}: {jq:?}");
        assert!(!jq[0].contains('\n') && jq[0].starts_with(".items"), "{profile:?}: {jq:?}");
    }
}

/// Nothing about variables is offered or drawn on the tab: no `{{` picker,
/// no picker by key, no extract, no token tint or tip, no `vars` chip.
#[test]
fn a_check_takes_no_variables() {
    for profile in PROFILES {
        let mut app = checks_app(FOUR, profile);
        assert!(!app.editor.caret_takes_vars(), "{profile:?}");
        feed(&mut app, "a");
        feed(&mut app, if profile == KeyProfile::Vim { "i{{" } else { "{{" });
        assert!(app.modals.is_empty(), "{profile:?}: `{{{{` opens no picker");
        retype(&mut app, "{{host}} == 1");
        draw(&mut app);
        let cell = app.hits.rect_of(&Hit::TableCell { row: 2, col: 0 }).unwrap();
        assert!(app.hits.var_token_at(cell.x + 1, cell.y).is_none(), "{profile:?}: no token");
        feed(&mut app, open(profile));
        assert_eq!(app.editor.caret_token(), None, "{profile:?}: no caret tip");
        for action in [
            Action::OpenVarPicker { completing: false },
            Action::ExtractToVariable,
            Action::ExtractToSelector,
        ] {
            app.toasts = Default::default();
            app.update(action.clone());
            assert!(app.modals.is_empty(), "{profile:?}: {action:?}");
            assert_eq!(app.toasts.last_message(), Some("a check takes no variables"));
        }
        let labels: Vec<String> =
            crate::ui::footer_chips_of(&app).into_iter().map(|c| c.1).collect();
        assert!(!labels.iter().any(|l| l == "vars"), "{profile:?}: {labels:?}");
        // The open check's text menu has Copy and Paste alone (vim's right
        // press on an open field extends its selection instead).
        click(&mut app, Hit::TableCell { row: 2, col: 0 }, Right);
        if profile == KeyProfile::Arrows {
            let Some(Modal::Dropdown(menu)) = app.modals.top() else { panic!("the text menu") };
            let items: Vec<&str> = menu.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(items, ["Copy", "Paste"]);
        }
    }
}

/// Q6: a check that doesn't compile is kept, saved, and drawn in the
/// error colour.
#[test]
fn a_check_that_does_not_compile_is_kept_in_the_error_colour() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = checks_app(FOUR, KeyProfile::Arrows);
    feed(&mut app, "a");
    retype(&mut app, ".a ==");
    assert_eq!(checks(&app).jq, [".a", ".b", ".a =="]);
    // Narrow too: both tabs paint inside a small pane.
    for (w, h) in [(160, 48), (66, 16), (30, 10)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    }
    draw(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let fg = |row: usize| {
        let r = app.hits.rect_of(&Hit::TableCell { row, col: 0 }).unwrap();
        terminal.backend().buffer().cell((r.x, r.y)).unwrap().fg
    };
    assert_eq!(fg(2), app.theme.error);
    assert_ne!(fg(0), app.theme.error);
}

/// A check a hand-written file spreads over lines is the row as read, and
/// a save that didn't touch it writes it back as it was.
#[test]
fn a_multi_line_check_from_a_file_is_written_back_as_read() {
    let toml = "url = \"http://x.test/\"\n[checks]\njq = [\"\"\".a\n  | . == 2\"\"\"]\n";
    let mut app = checks_app(toml, KeyProfile::Arrows);
    assert_eq!(checks(&app).jq, [".a\n  | . == 2"]);
    app.editor.table.set_selected(Some(Stop::Row(0)));
    feed(&mut app, "<CR><CR>");
    assert!(!app.editor.is_dirty(), "opened and closed untouched");
    assert_eq!(checks(&app).jq, [".a\n  | . == 2"]);
}

/// A send of the open request that brings a response with `body`, handed
/// over as the send's task hands it: tracked in flight, then
/// `ResponseArrived` with its checks as they stand on arrival. Answers
/// the send's generation and the lines `checks` give, for `ChecksJudged`.
fn send_arrives(app: &mut App, status: u16, body: &str, checks: &Checks) -> (u64, Lines) {
    use crate::components::response::CheckState;
    let data = crate::http::ResponseData {
        status,
        url: "http://x.test/".into(),
        headers: vec![],
        sent_headers: vec![],
        body: body.to_string(),
        ttfb: std::time::Duration::from_millis(5),
        elapsed: std::time::Duration::from_millis(9),
        size: body.len(),
        content_type: None,
    };
    let slug = app.editor.slug.clone();
    let generation = app.session.begin_send(&slug);
    let (started, task) = (Instant::now(), tokio::spawn(async {}));
    app.session.in_flight.push(crate::session::InFlight { started, generation, slug, task });
    let (state, lines) = (CheckState::on_arrival(&data, checks), data.check_lines(checks));
    app.update(Action::ResponseArrived { generation, data: Box::new(data), checks: state });
    draw(app);
    (generation, lines)
}

type Lines = Vec<(bool, String)>;

/// [`send_arrives`], and then its jq checks' lines landed.
fn arrive(app: &mut App, status: u16, body: &str, checks: &Checks) {
    let (generation, lines) = send_arrives(app, status, body, checks);
    if !checks.jq.is_empty() {
        app.update(Action::ChecksJudged { generation, lines });
        draw(app);
    }
}

fn shown(app: &App) -> String {
    app.session.response.view().map(|v| v.view_text()).unwrap_or_default()
}

fn response_modes(app: &App) -> Vec<crate::components::response::ViewMode> {
    app.session.response.tab_modes().unwrap_or_default()
}

/// P1, P5, P6: the tab is the response's. It shows for a response with
/// outcomes, stays as it was when the checks are edited afterwards and
/// across a switch to another request and back, and a failed send or a
/// request without checks has none.
#[tokio::test]
async fn the_results_are_the_responses_and_survive_edits_and_a_switch() {
    use crate::components::response::ViewMode;
    let mut app = checks_app(FOUR, KeyProfile::Arrows);
    let sent = checks(&app);
    arrive(&mut app, 500, r#"{"a":true,"b":1}"#, &sent);
    assert!(response_modes(&app).contains(&ViewMode::Checks));
    assert_ne!(app.session.response.view().unwrap().mode, ViewMode::Checks, "no tab switched");
    assert!(app.toasts.last_message().is_none(), "a failed check shows no toast");
    app.session.response.set_view_mode(ViewMode::Checks);
    let lines = "✗ status 500, expected 200\n✓ total 9 ms, limit 2000 ms\n✓ .a\n✗ .b gave 1";
    assert_eq!(shown(&app), lines);
    // Editing the checks afterwards re-judges nothing.
    app.editor.table.set_selected(Some(Stop::Row(1)));
    feed(&mut app, "d");
    feed(&mut app, "<Up><Up><Up><CR>");
    retype(&mut app, "");
    assert_eq!((checks(&app).status.len(), checks(&app).jq.len()), (0, 1));
    assert_eq!(shown(&app), lines, "the results are that response's");
    // Away to another request and back.
    app.update(Action::SaveRequest);
    let other = HttpRequest::from_toml_str("url = \"http://y.test/\"\n").unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/other", &other).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/other".into()));
    assert!(app.session.response.view().is_none(), "the other request has no response");
    app.update(Action::ForceOpenRequest("main/probe".into()));
    assert_eq!(shown(&app), lines, "still there after a switch away and back");
    // A request with no checks, and a failed send: no tab.
    arrive(&mut app, 500, "oops", &Checks::default());
    assert!(!response_modes(&app).contains(&ViewMode::Checks));
    let generation = app.session.send_generation;
    app.session.response.set_state(ResponseState::Failed("refused".into()), generation);
    assert!(response_modes(&app).is_empty());
}

/// Section 7, A1 to A4 and A7: the response shows as it arrives, with its
/// Checks tab on `checking…` and a dim mark; copy and search act on that
/// line; then the lines and the verdict take their place, with no toast,
/// no tab switched and nothing on the tab row moved. Only a status check
/// and a limit: judged as it arrives.
#[tokio::test]
async fn a_response_shows_at_once_and_its_checks_tab_fills_in_after() {
    use super::tests::{last_copied, rendered_text_at};
    use crate::components::response::ViewMode;
    for profile in PROFILES {
        let mut app = checks_app(FOUR, profile);
        let sent = checks(&app);
        let (generation, lines) = send_arrives(&mut app, 500, r#"{"a":true,"b":1}"#, &sent);
        assert!(matches!(app.session.response.state(), ResponseState::Ready(_)), "{profile:?}");
        assert!(response_modes(&app).contains(&ViewMode::Checks));
        assert_ne!(app.session.response.view().unwrap().mode, ViewMode::Checks);
        assert!(rendered_text_at(&mut app, 120, 30).contains("Checks \u{2026}"), "{profile:?}");
        let tab = Hit::ResponseTab(ViewMode::Checks);
        let waiting = app.hits.rect_of(&tab);
        app.update(Action::ResponseViewMode(ViewMode::Checks));
        assert_eq!(shown(&app), "checking\u{2026}");
        app.update(Action::CopyToClipboard(crate::action::CopyTarget::ResponseView));
        assert_eq!(last_copied(&app).as_deref(), Some("checking\u{2026}"));
        assert_eq!(app.toasts.last_message(), Some("Copied check results"));
        app.focus = PaneId::Response;
        app.update(Action::OpenResponseSearch { backward: false });
        feed(&mut app, "check<CR>");
        let found =
            |app: &App| app.session.response.view().unwrap().search.as_ref().unwrap().matches.len();
        assert_eq!(found(&app), 1, "{profile:?}");
        app.toasts = Default::default();
        // The lines land.
        assert!(app.update(Action::ChecksJudged { generation, lines }));
        assert_eq!(app.session.response.view().unwrap().mode, ViewMode::Checks, "{profile:?}");
        assert!(app.toasts.last_message().is_none(), "no toast");
        let lines = "✗ status 500, expected 200\n✓ total 9 ms, limit 2000 ms\n✓ .a\n✗ .b gave 1";
        assert_eq!(shown(&app), lines);
        assert_eq!(found(&app), 0, "the search is over the lines now");
        assert!(rendered_text_at(&mut app, 120, 30).contains("Checks \u{2717}"), "{profile:?}");
        assert_eq!(app.hits.rect_of(&tab), waiting, "the tab row did not move");
        // Landing on another tab switches nothing.
        let (generation, lines) = send_arrives(&mut app, 200, r#"{"a":true,"b":true}"#, &sent);
        let mode = app.session.response.view().unwrap().mode;
        assert!(app.update(Action::ChecksJudged { generation, lines }));
        assert_eq!(app.session.response.view().unwrap().mode, mode);
        assert!(rendered_text_at(&mut app, 120, 30).contains("Checks \u{2713}"), "{profile:?}");
        // No jq check: nothing to wait for.
        let quick = Checks { jq: Vec::new(), ..sent };
        send_arrives(&mut app, 200, "{}", &quick);
        app.update(Action::ResponseViewMode(ViewMode::Checks));
        assert_eq!(shown(&app), "✓ status 200\n✓ total 9 ms, limit 2000 ms", "{profile:?}");
    }
}

/// Section 7, A5: the lines are their own response's. They land on it
/// while another request is on screen and leave that one's response alone;
/// once a new send has replaced the response (in flight, arrived or
/// failed) they are dropped, and a deleted request's land on nothing shown.
#[tokio::test]
async fn check_lines_land_on_their_own_response_or_nowhere() {
    use crate::components::response::ViewMode;
    let mut app = checks_app(FOUR, KeyProfile::Arrows);
    let sent = checks(&app);
    let other = HttpRequest::from_toml_str("url = \"http://y.test/\"\n").unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/other", &other).unwrap();
    app.update(Action::RefreshSidebar);
    let (first, lines) = send_arrives(&mut app, 200, r#"{"a":true,"b":true}"#, &sent);
    let judged = "✓ status 200\n✓ total 9 ms, limit 2000 ms\n✓ .a\n✓ .b";
    // Another request on screen, with a response of its own.
    app.update(Action::ForceOpenRequest("main/other".into()));
    send_arrives(&mut app, 200, "theirs", &Checks::default());
    assert!(app.update(Action::ChecksJudged { generation: first, lines: lines.clone() }));
    assert_eq!(shown(&app), "theirs");
    assert!(!response_modes(&app).contains(&ViewMode::Checks), "not the other request's");
    app.update(Action::ForceOpenRequest("main/probe".into()));
    app.update(Action::ResponseViewMode(ViewMode::Checks));
    assert_eq!(shown(&app), judged, "there on the way back");
    // A second send while the first send's checks run.
    let (stale, _) = send_arrives(&mut app, 200, r#"{"a":true,"b":true}"#, &sent);
    let (second, _) = send_arrives(&mut app, 200, r#"{"a":1}"#, &sent);
    assert!(!app.update(Action::ChecksJudged { generation: stale, lines: lines.clone() }));
    app.update(Action::ResponseViewMode(ViewMode::Checks));
    assert_eq!(shown(&app), "checking\u{2026}", "the old result never lands on the new response");
    // A send in flight, a failed one and a cancelled one hold no response.
    let slug = app.editor.slug.clone();
    let third = app.session.begin_send(&slug);
    assert!(!app.update(Action::ChecksJudged { generation: second, lines: lines.clone() }));
    let (started, task) = (Instant::now(), tokio::spawn(async {}));
    let flight = crate::session::InFlight { started, generation: third, slug, task };
    app.session.in_flight.push(flight);
    app.update(Action::RequestFailed { generation: third, error: "refused".into() });
    assert!(!app.update(Action::ChecksJudged { generation: third, lines: lines.clone() }));
    assert!(matches!(app.session.response.state(), ResponseState::Failed(_)));
    // The request deleted while its checks run.
    let (last, _) = send_arrives(&mut app, 200, r#"{"a":true,"b":true}"#, &sent);
    app.update(Action::DeleteRequest("main/probe".into()));
    assert_ne!(app.editor.slug.as_deref(), Some("main/probe"));
    let before = shown(&app);
    app.update(Action::ChecksJudged { generation: last, lines });
    assert_eq!(shown(&app), before, "nothing lands on the request now open");
}
