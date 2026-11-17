//! The Manage screen's Project tab (spec 2026-11-15 project settings):
//! the name and the default headers, written to `project.toml` at once
//! as one undo step each, in both key profiles.

use super::key_matrix_tests::{draw, feed, use_persisted_profile};
use super::*;
use crate::components::manage::ManageTab;
use crate::components::table_editor::Stop;
use crate::config::KeyProfile;
use crate::keyroute::FieldId;

/// An app on a new project whose `project.toml` is `toml`.
pub(super) fn app_with_project_toml(toml: &str) -> App {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    postui_core::fixtures::init_project(root, Some("matrix")).unwrap();
    std::fs::write(root.join("project.toml"), toml).unwrap();
    postui_core::fixtures::save_local_state(root, &postui_core::project::LocalState::default())
        .unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, root.to_path_buf());
    app._test_rx = Some(rx);
    app._test_dir = Some(dir);
    app
}

/// An app on a project whose `project.toml` is `toml`, the Project tab
/// open in `profile`.
pub(super) fn project_app(toml: &str, profile: KeyProfile) -> App {
    let mut app = app_with_project_toml(toml);
    use_persisted_profile(&mut app, profile);
    app.update(Action::OpenManage { tab: Some(ManageTab::Project) });
    draw(&mut app);
    app
}

const THREE: &str = "name = \"Orders\"\n\n[default_headers]\na = \"1\"\nb = \"2\"\nc = \"3\"\n";

fn file(app: &App) -> String {
    std::fs::read_to_string(app.proj().root().join("project.toml")).unwrap()
}

fn keys(app: &App) -> Vec<String> {
    app.proj().meta().default_headers.keys().cloned().collect()
}

fn undo_key(profile: KeyProfile) -> &'static str {
    if profile == KeyProfile::Vim { "u" } else { "<C-z>" }
}

/// `hit`, a request table's hit, as the Project table registers it.
fn pt(hit: Hit) -> Hit {
    use crate::components::table_editor::TableOwner;
    TableOwner::Project.hit(TableOwner::Request.of(&hit).expect("a table hit"))
}

fn cell(row: usize, col: u8) -> Hit {
    pt(Hit::TableCell { row, col })
}

/// The table's hits are its own: none registers as the request table's.
#[test]
fn the_tab_shows_the_name_and_the_headers() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    assert_eq!(app.surface(), crate::keyroute::Surface::Project);
    draw(&mut app);
    assert!(app.hits.rect_of(&Hit::ProjectName).is_some());
    for row in 0..3 {
        assert!(app.hits.rect_of(&cell(row, 1)).is_some(), "row {row}");
        assert!(app.hits.rect_of(&Hit::TableCell { row, col: 1 }).is_none(), "row {row}");
    }
    assert!(app.hits.rect_of(&cell(3, 0)).is_some(), "+ Add header");
}

/// R5–R7: Enter opens the name seeded with it; Enter and Esc both commit,
/// trimmed; one undo step each, which puts the file back.
#[test]
fn the_name_commits_on_enter_and_esc_trimmed_and_undoes() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(THREE, profile);
        let before = file(&app);
        let open = if profile == KeyProfile::Vim { "i" } else { "<CR>" };
        feed(&mut app, open);
        assert_eq!(app.field_caret(), crate::keyroute::FieldCaret::Live(FieldId::ProjectName));
        let typed = if profile == KeyProfile::Vim { "<C-u>  Billing API <Esc><Esc>" } else { "" };
        if profile == KeyProfile::Arrows {
            app.project_tab.name_input_mut().unwrap().set_text("  Billing API ");
            feed(&mut app, "<CR>");
        } else {
            feed(&mut app, typed);
        }
        assert!(!app.project_tab.name_open(), "{profile:?}: closed");
        assert_eq!(app.proj().meta().name.as_deref(), Some("Billing API"), "{profile:?}");
        assert!(file(&app).contains("name = \"Billing API\""), "{profile:?}: {}", file(&app));
        feed(&mut app, undo_key(profile));
        assert_eq!(file(&app), before, "{profile:?}: undo restores the file");
        assert_eq!(app.proj().display_name(), "Orders");
    }
}

#[test]
fn esc_commits_the_name_too() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("Esc kept");
    feed(&mut app, "<Esc>");
    assert_eq!(app.proj().meta().name.as_deref(), Some("Esc kept"));
    assert_eq!(app.screen, Screen::Manage, "the Esc closed the field, not the screen");
}

/// R7: a cleared name removes the key, and the project shows its folder.
#[test]
fn a_cleared_name_removes_the_key() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("   ");
    feed(&mut app, "<CR>");
    assert_eq!(app.proj().meta().name, None);
    assert!(!file(&app).contains("name ="), "{}", file(&app));
    let folder = app.proj().root().file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(app.proj().display_name(), folder);
}

/// An unchanged name writes nothing and records no step.
#[test]
fn an_unchanged_name_writes_nothing() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    let before = file(&app);
    feed(&mut app, "<CR><CR>");
    assert_eq!(file(&app), before);
    assert!(!app.proj().can_undo());
}

/// R8–R10: `a` opens `+ Add header`; the name, then the value, each
/// written as it commits; the new row goes last.
#[test]
fn a_header_is_added_last_and_each_cell_is_a_step() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(THREE, profile);
        let before = file(&app);
        feed(&mut app, "j");
        if profile == KeyProfile::Arrows {
            // Arrows' `j` is no move; the arrow is.
            feed(&mut app, "<Down>");
        }
        feed(&mut app, "a");
        assert!(app.project_tab.table.editing_ghost(3), "{profile:?}: + Add header opened");
        feed(&mut app, "X-Trace<Tab>on");
        assert_eq!(keys(&app), ["a", "b", "c", "X-Trace"], "{profile:?}: the name was written");
        feed(&mut app, if profile == KeyProfile::Vim { "<Esc><Esc>" } else { "<CR>" });
        assert_eq!(app.proj().meta().default_headers["X-Trace"].value, "on", "{profile:?}");
        assert!(file(&app).contains("X-Trace = \"on\""), "{profile:?}: {}", file(&app));
        feed(&mut app, undo_key(profile));
        assert_eq!(app.proj().meta().default_headers["X-Trace"].value, "", "{profile:?}");
        feed(&mut app, undo_key(profile));
        assert_eq!(file(&app), before, "{profile:?}: two steps undo the add");
    }
}

/// R8: Space toggles a row off, written as `{ value, enabled = false }`.
#[test]
fn space_turns_a_header_off() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><Down><Space>");
    assert!(!app.proj().meta().default_headers["b"].enabled);
    assert!(file(&app).contains("b = { value = \"2\", enabled = false }"), "{}", file(&app));
    feed(&mut app, "<C-z>");
    assert!(app.proj().meta().default_headers["b"].enabled);
}

/// R8, R9: delete on a row, one step, no confirm; the cursor stays on the
/// row that moved up.
#[test]
fn a_header_is_deleted_in_one_step() {
    for (profile, del) in
        [(KeyProfile::Arrows, "d"), (KeyProfile::Arrows, "<Del>"), (KeyProfile::Vim, "dd")]
    {
        let mut app = project_app(THREE, profile);
        let before = file(&app);
        let down = if profile == KeyProfile::Vim { "jj" } else { "<Down><Down>" };
        feed(&mut app, down);
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(1)), "{profile:?} {del}");
        feed(&mut app, del);
        assert_eq!(keys(&app), ["a", "c"], "{profile:?} {del}");
        assert!(app.modals.is_empty(), "no confirm");
        // The request table's toast: what went, and the key that undoes.
        let hint = app.undo_hint();
        assert!(hint.contains("undoes"), "{profile:?}: {hint:?}");
        assert_eq!(
            app.toasts.last_message(),
            Some(format!("Deleted row \"b\"{hint}")).as_deref(),
            "{profile:?} {del}"
        );
        feed(&mut app, undo_key(profile));
        assert_eq!(file(&app), before, "{profile:?} {del}");
    }
}

const FOUR: &str = "[default_headers]\na = \"1\"\nb = \"2\"\nc = \"3\"\nd = \"4\"\n";

/// A delete pill keeps the cursor on the header it was on: a delete above
/// it moves it up with its header, one below leaves it, and its own row's
/// pill lands it on the row that moved up, as the delete keys do.
#[test]
fn a_delete_pill_keeps_the_cursor_on_its_header() {
    // (the cursor's row, the pill's row, the header the cursor ends on)
    for (at, pill, on) in [(2, 1, "c"), (3, 1, "d"), (1, 3, "b"), (1, 1, "c")] {
        let mut app = project_app(FOUR, KeyProfile::Arrows);
        feed(&mut app, &"<Down>".repeat(at + 1));
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(at)));
        app.hovered = Some(cell(pill, 1));
        click(&mut app, pt(Hit::TableDelete(pill)));
        assert_eq!(keys(&app).len(), 3, "{at} {pill}");
        let row = app.project_tab.selected_row(3).unwrap_or_else(|| panic!("{at} {pill}: no row"));
        assert_eq!(keys(&app)[row], on, "{at} {pill}");
    }
}

/// R13: `y` (vim `yy`) copies the value of the header under the cursor,
/// as its copy pill does, and the footer offers it on a row.
#[test]
fn y_copies_a_header_value() {
    for (profile, yank) in [(KeyProfile::Arrows, "y"), (KeyProfile::Vim, "yy")] {
        let mut app = project_app(THREE, profile);
        feed(&mut app, if profile == KeyProfile::Vim { "jj" } else { "<Down><Down>" });
        let labels: Vec<&str> =
            app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
        assert!(labels.contains(&"copy"), "{profile:?}: {labels:?}");
        feed(&mut app, yank);
        assert_eq!(super::tests::last_copied(&app).as_deref(), Some("2"), "{profile:?}");
        assert_eq!(app.toasts.last_message(), Some("Copied b"), "{profile:?}");
    }
}

/// R5: a blank `name` in the file is no name: the Name row shows the
/// folder name, as with none set, and so does the project's name.
#[test]
fn a_blank_name_shows_the_folder_name() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = project_app("name = \"  \"\n", KeyProfile::Arrows);
    let folder = app.proj().folder_name();
    assert_eq!(app.proj().display_name(), folder);
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let r = app.hits.rect_of(&Hit::ProjectName).unwrap();
    let buf = terminal.backend().buffer();
    let well: String = (r.x..r.right()).map(|x| buf[(x, r.y)].symbol().to_string()).collect();
    assert!(well.contains(&folder), "{well:?}");
}

/// A `name` written by hand with spaces round it shows trimmed at rest,
/// as a commit would write it, and opens trimmed.
#[test]
fn a_padded_name_shows_trimmed() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let well = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let r = app.hits.rect_of(&Hit::ProjectName).unwrap();
        let buf = terminal.backend().buffer();
        (r.x..r.right()).map(|x| buf[(x, r.y)].symbol().to_string()).collect::<String>()
    };
    let mut app = project_app("name = \"  Orders  \"\n", KeyProfile::Arrows);
    assert_eq!(app.proj().display_name(), "Orders");
    let mut plain = project_app("name = \"Orders\"\n", KeyProfile::Arrows);
    assert_eq!(well(&mut app), well(&mut plain));
    feed(&mut app, "<CR>");
    assert_eq!(
        app.project_tab.name_input_mut().map(|i| i.text().to_string()),
        Some("Orders".into())
    );
}

/// R11: renaming a header onto a name already there merges the two, as
/// in a request's Headers table: one row stays, in its place, with the
/// renamed row's value, and a warning says so.
#[test]
fn a_rename_onto_a_taken_name_merges_with_a_warning() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><CR>");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("b");
    feed(&mut app, "<CR>");
    assert_eq!(keys(&app), ["b", "c"]);
    assert_eq!(app.proj().meta().default_headers["b"].value, "1");
    let messages = app.toasts.messages();
    assert!(messages.iter().any(|m| m.contains("duplicate key 'b'")), "{messages:?}");
    feed(&mut app, "<C-z>");
    assert_eq!(keys(&app), ["a", "b", "c"], "one undo step");
}

/// R17 (user, 2026-11-16): a change on the tab writes the whole
/// `[default_headers]` table as the tab shows it, so a header added by
/// hand since the app last read the file is overwritten; undo puts the
/// hand edit back.
#[test]
fn a_change_overwrites_a_hand_edit_and_undo_restores_it() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    let hand = format!("{THREE}d = \"4\"\n");
    std::fs::write(app.proj().root().join("project.toml"), &hand).unwrap();
    feed(&mut app, "<Down><Down><Space>");
    assert_eq!(keys(&app), ["a", "b", "c"]);
    assert!(!file(&app).contains("\nd = "), "{}", file(&app));
    feed(&mut app, "<C-z>");
    assert_eq!(file(&app), hand, "undo restores the hand edit");
}

/// #310 spec §3: nothing on the tab writes `jq_limit_ms`, and a change
/// there keeps it.
#[test]
fn a_change_keeps_the_jq_limit() {
    let mut app = project_app(&format!("jq_limit_ms = 250\n{THREE}"), KeyProfile::Arrows);
    feed(&mut app, "<Down><Down><Space>");
    assert_ne!(file(&app), format!("jq_limit_ms = 250\n{THREE}"), "the tab wrote");
    assert!(file(&app).starts_with("jq_limit_ms = 250\n"), "{}", file(&app));
    assert_eq!(app.meta().jq_limit(), std::time::Duration::from_millis(250));
}

/// R12: ↑ off the first row and Home go back to the Name row; Esc with
/// nothing open leaves the screen.
#[test]
fn the_cursor_walks_between_the_name_and_the_table() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    assert!(app.project_tab.on_name());
    feed(&mut app, "<Down>");
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)));
    feed(&mut app, "<Up>");
    assert!(app.project_tab.on_name());
    feed(&mut app, "<End>");
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Add));
    feed(&mut app, "<Home>");
    assert!(app.project_tab.on_name());
    feed(&mut app, "<Esc>");
    assert_eq!(app.screen, Screen::Main);
}

/// R12 (comment 1382): `a` adds a header from anywhere on the tab, the
/// Name row included, and the footer offers add there.
#[test]
fn a_on_the_name_row_adds_a_header() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(THREE, profile);
        assert!(app.project_tab.on_name(), "{profile:?}");
        let labels: Vec<&str> =
            app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
        assert!(labels.contains(&"add"), "{profile:?}: {labels:?}");
        feed(&mut app, "a");
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Add), "{profile:?}");
        assert!(app.project_tab.table.editing_ghost(3), "{profile:?}: + Add header opened");
        feed(&mut app, "X-New");
        feed(&mut app, if profile == KeyProfile::Vim { "<Esc><Esc>" } else { "<CR>" });
        assert_eq!(keys(&app), ["a", "b", "c", "X-New"], "{profile:?}");
    }
}

/// The tab's cursor is drawn only while the tab is on screen: leaving the
/// Manage screen clears it, and coming back to the tab hands it back.
#[test]
fn leaving_the_screen_clears_the_cursor() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    assert!(app.project_tab.focused);
    feed(&mut app, "<Esc>");
    assert_eq!(app.screen, Screen::Main);
    assert!(!app.project_tab.focused);
    app.update(Action::OpenManage { tab: Some(ManageTab::Project) });
    assert!(app.project_tab.focused);
}

/// R14: a tab switch commits an open cell first.
#[test]
fn a_tab_switch_commits_an_open_cell() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><Right><CR>");
    let input = &mut app.project_tab.table.editing.as_mut().expect("the value is open").input;
    input.set_text("one");
    app.update(Action::SelectManageTab(ManageTab::Spaces));
    assert_eq!(app.manage.tab, ManageTab::Spaces);
    assert_eq!(app.proj().meta().default_headers["a"].value, "one");
    assert!(app.project_tab.table.editing.is_none());
}

/// R14: a write `project.toml` refuses keeps the field open with the
/// typing, and says why.
#[test]
fn a_refused_write_keeps_the_typing() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><Right><CR>");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("kept");
    std::fs::write(app.proj().root().join("project.toml"), "name = \n").unwrap();
    feed(&mut app, "<CR>");
    let edit = app.project_tab.table.editing.as_ref().expect("still open");
    assert_eq!(edit.input.text(), "kept");
    assert!(
        app.toasts.messages().iter().any(|m| m.contains("cannot save default headers")),
        "{:?}",
        app.toasts.messages()
    );
    assert!(!app.op_in_flight, "the refused write ends its op");
    // The name is refused the same way.
    app.project_tab.table.editing = None;
    app.project_tab.table.set_selected(Some(Stop::Head(0)));
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("New");
    feed(&mut app, "<CR>");
    assert_eq!(app.project_tab.name_input_mut().map(|i| i.text().to_string()), Some("New".into()));
    assert!(!app.op_in_flight, "the refused name ends its op");
}

/// R7's one rule: a commit writes only when the trimmed typing (or no
/// name, when empty) differs from the name the row opened with, the
/// file's `name` trimmed. So a padded or blank `name` opened and left
/// unchanged stays as written, with no undo step, whether Enter, Esc or a
/// click away closes it; a changed one is written trimmed, one undo step.
#[test]
fn an_unchanged_padded_or_blank_name_writes_nothing() {
    for toml in ["name = \" Orders \"\n", "name = \"\"\n", "name = \"  \"\n"] {
        for close in ["<CR>", "<Esc>", "click"] {
            let mut app = project_app(toml, KeyProfile::Arrows);
            let before = file(&app);
            feed(&mut app, "<CR>");
            assert!(app.project_tab.name_open(), "{toml} {close}");
            if close == "click" {
                click(&mut app, Hit::ManageTab(ManageTab::Project.index()));
            } else {
                feed(&mut app, close);
            }
            assert!(!app.project_tab.name_open(), "{toml} {close}");
            assert_eq!(file(&app), before, "{toml} {close}");
            assert!(!app.proj().can_undo(), "{toml} {close}: no step");
        }
        let mut app = project_app(toml, KeyProfile::Arrows);
        let before = file(&app);
        feed(&mut app, "<CR>");
        app.project_tab.name_input_mut().unwrap().set_text(" Billing ");
        feed(&mut app, "<CR>");
        assert_eq!(app.proj().meta().name.as_deref(), Some("Billing"), "{toml}");
        feed(&mut app, "<C-z>");
        assert_eq!(file(&app), before, "{toml}: one undo step");
    }
}

/// R7: a name that differs from the file's only by spaces round it names
/// the same project, so it starts no write at all: one the file would
/// refuse is never tried, and the name closes.
#[test]
fn a_name_differing_only_by_spaces_starts_no_write() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("Orders ");
    std::fs::write(app.proj().root().join("project.toml"), "name = \n").unwrap();
    feed(&mut app, "<CR>");
    assert!(!app.project_tab.name_open(), "the name closed");
    assert!(app.toasts.messages().is_empty(), "{:?}", app.toasts.messages());
    assert!(!app.op_in_flight);
}

/// A key the tab hands on leaves its cursor as it was: after a click away
/// dropped it, Tab, ← or → on the Name row does nothing here and lights
/// nothing.
#[test]
fn a_key_the_tab_hands_on_leaves_the_cursor_dropped() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    click(&mut app, Hit::ManageTab(ManageTab::Project.index()));
    assert!(!app.project_tab.focused);
    assert!(app.project_tab.on_name());
    for code in [KeyCode::Tab, KeyCode::Left, KeyCode::Right] {
        assert_eq!(app.handle_project_key(KeyEvent::new(code, KeyModifiers::NONE)), None);
        assert!(!app.project_tab.focused, "{code:?}: the unclaimed key relit the cursor");
    }
}

/// R14: a right click away from an open header cell commits it first, as
/// a left click does.
#[test]
fn a_right_click_away_commits_an_open_cell() {
    let mut app = open_b_value("22");
    draw(&mut app);
    let r = app.hits.rect_of(&Hit::ProjectName).unwrap();
    app.handle_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Right,
        ),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.project_tab.table.editing.is_none(), "the cell closed");
    assert_eq!(app.proj().meta().default_headers["b"].value, "22");
}

fn click(app: &mut App, hit: Hit) {
    draw(app);
    let r = app.hits.rect_of(&hit).unwrap_or_else(|| panic!("no {hit:?}"));
    click_at(app, r.x + 1, r.y);
}

/// A left click at `(x, y)` on the last frame.
fn click_at(app: &mut App, x: u16, y: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        app.handle_mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    }
    app.capture_undo();
    draw(app);
}

/// A click on header `i`'s row itself: the divider between its cells,
/// where the row's own hit is the only one.
fn click_row(app: &mut App, i: usize) {
    draw(app);
    let r = app.hits.rect_of(&cell(i, 0)).unwrap_or_else(|| panic!("no row {i}"));
    assert_eq!(app.hits.hit_at(r.right(), r.y), Some(&pt(Hit::TableRow(i))));
    click_at(app, r.right(), r.y);
}

/// The mouse reaches what the keys do: a cell opens in place, the
/// checkbox toggles, the delete pill deletes, the name well opens, and a
/// click away commits.
#[test]
fn clicks_edit_toggle_and_delete() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    click(&mut app, cell(2, 1));
    assert_eq!(
        app.field_caret(),
        crate::keyroute::FieldCaret::Live(FieldId::ProjectCell { row: 2, col: 1 })
    );
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("33");
    click(&mut app, pt(Hit::TableCheckbox(0)));
    assert_eq!(app.proj().meta().default_headers["c"].value, "33", "the click away committed");
    assert!(!app.proj().meta().default_headers["a"].enabled, "and the checkbox toggled");
    // The row's actions show under the pointer.
    app.hovered = Some(cell(1, 1));
    click(&mut app, pt(Hit::TableDelete(1)));
    assert_eq!(keys(&app), ["a", "c"]);
    click(&mut app, Hit::ProjectName);
    assert!(app.project_tab.name_open());
    // The Manage chip closes the screen, committing the name first.
    app.project_tab.name_input_mut().unwrap().set_text("Clicked");
    click(&mut app, Hit::ManageTab(ManageTab::Spaces.index()));
    assert_eq!(app.proj().meta().name.as_deref(), Some("Clicked"));
}

/// The checkbox, copy and delete pills act on their header and leave the
/// tab's cursor where it was, as a request table's checkbox does: from the
/// Name row the Name row stays lit and the table gets no cursor, and on
/// the table the cursor keeps its row. The keys still act after.
#[test]
fn the_pills_leave_the_cursor_where_it_was() {
    let pills: [fn(usize) -> Hit; 3] = [Hit::TableCheckbox, Hit::TableCopy, Hit::TableDelete];
    for pill in pills {
        let hit = pt(pill(1));
        let mut app = project_app(THREE, KeyProfile::Arrows);
        app.hovered = Some(cell(1, 1));
        click(&mut app, hit.clone());
        assert!(app.project_tab.on_name(), "{hit:?}: on the Name row");
        assert!(app.project_tab.focused, "{hit:?}: the Name row is lit");
        feed(&mut app, "<Down>");
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)), "{hit:?}");
        let labels: Vec<&str> =
            app.project_tab.footer_chips(keys(&app).len()).iter().map(|(_, l, _)| *l).collect();
        assert!(labels.contains(&"delete"), "{hit:?}: {labels:?}");

        // On the table, the cursor keeps its row.
        let mut app = project_app(THREE, KeyProfile::Arrows);
        feed(&mut app, "<Down>");
        app.hovered = Some(cell(1, 1));
        click(&mut app, hit.clone());
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)), "{hit:?}");
    }
}

/// R18: a click whose own commit took a header away (b renamed onto d,
/// so the rows under b move up one) does nothing, whatever it lands on:
/// a cell, a row, the checkbox, the copy pill or the delete pill. The
/// rename is written, and the next click acts.
#[test]
fn every_click_after_a_merging_rename_does_nothing() {
    let kinds: [(&str, Hit); 5] = [
        ("cell", cell(2, 1)),
        ("row", pt(Hit::TableRow(0))),
        ("checkbox", pt(Hit::TableCheckbox(2))),
        ("copy", pt(Hit::TableCopy(2))),
        ("delete", pt(Hit::TableDelete(2))),
    ];
    let press = |app: &mut App, kind: &str, hit: &Hit| match kind {
        "row" => click_row(app, 0),
        _ => click(app, hit.clone()),
    };
    for (kind, hit) in kinds {
        let mut app = project_app(FOUR, KeyProfile::Arrows);
        click(&mut app, cell(1, 0));
        app.project_tab.table.editing.as_mut().expect("b's name is open").input.set_text("d");
        app.hovered = Some(cell(2, 1));
        press(&mut app, kind, &hit);
        assert_eq!(keys(&app), ["a", "c", "d"], "{kind}: merged, nothing deleted");
        let headers = &app.proj().meta().default_headers;
        assert!(headers.values().all(|e| e.enabled), "{kind}: nothing toggled");
        assert_eq!(super::tests::last_copied(&app), None, "{kind}: nothing copied");
        assert!(app.project_tab.field().is_none(), "{kind}: nothing opened");
        let at = app.project_tab.table.selected();
        assert_eq!(at, Some(Stop::Row(2)), "{kind}: the cursor stays on the merged row");
        // The next click acts.
        app.hovered = Some(cell(2, 1));
        press(&mut app, kind, &hit);
        let acted = match kind {
            "cell" => app.project_tab.field() == Some(FieldId::ProjectCell { row: 2, col: 1 }),
            "row" => app.project_tab.table.selected() == Some(Stop::Row(0)),
            "checkbox" => !app.proj().meta().default_headers["d"].enabled,
            "copy" => super::tests::last_copied(&app).as_deref() == Some("2"),
            _ => keys(&app) == ["a", "c"],
        };
        assert!(acted, "{kind}: the next click acts");
    }
}

/// R18: an add puts its header last, where `+ Add header` was, so the rows
/// above stay put and a click after it acts: a name typed in `+ Add` and a
/// click on its value cell open the new header's value in one click, and a
/// checkbox click toggles the header it lands on.
#[test]
fn a_click_after_an_add_acts() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "a");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("X-New");
    click(&mut app, cell(3, 1));
    assert_eq!(keys(&app), ["a", "b", "c", "X-New"]);
    assert_eq!(app.project_tab.field(), Some(FieldId::ProjectCell { row: 3, col: 1 }));

    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "a");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("X-New");
    click(&mut app, pt(Hit::TableCheckbox(0)));
    assert_eq!(keys(&app), ["a", "b", "c", "X-New"]);
    assert!(!app.proj().meta().default_headers["a"].enabled, "the checkbox toggled");
}

/// R16: an undo that takes away the row under the cursor leaves it on the
/// last row still there, and the row verbs act on that row.
#[test]
fn an_undo_that_removes_rows_clamps_the_cursor() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down>aX<CR>");
    assert_eq!(keys(&app), ["a", "b", "c", "X"]);
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(3)));
    feed(&mut app, "<C-z>");
    assert_eq!(keys(&app), ["a", "b", "c"]);
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(2)));
    let labels: Vec<&str> = app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
    assert!(labels.contains(&"delete"), "{labels:?}");
    feed(&mut app, "d");
    assert_eq!(keys(&app), ["a", "b"]);
}

/// The tips over the table's checkbox and copy pill read the default
/// header, not the open request's row of the same index.
#[test]
fn the_table_tips_read_the_default_headers() {
    let toml = "[default_headers]\na = { value = \"1\", enabled = false }\nb = \"2\"\n";
    let mut app = project_app(toml, KeyProfile::Arrows);
    app.editor.active_tab = crate::components::editor::EditorTab::Headers;
    let tip = |app: &App, hit: Hit| crate::hint::hint_for(&hit, &app.keymap, &app.hint_ctx(&hit));
    assert_eq!(tip(&app, pt(Hit::TableCheckbox(0))).as_deref(), Some("Enable this row"));
    assert_eq!(tip(&app, pt(Hit::TableCheckbox(1))).as_deref(), Some("Disable this row"));
    // Each pill's tip is the action its click runs, the footer chip's own.
    for (pill, action) in [
        (Hit::TableCopy(1), Action::CopyDefaultHeader(1)),
        (Hit::TableDelete(1), Action::DeleteDefaultHeader(1)),
    ] {
        let chip = Hit::FooterChip(action);
        let pill_tip = tip(&app, pt(pill.clone()));
        assert!(pill_tip.is_some(), "{pill:?}");
        assert_eq!(pill_tip, tip(&app, chip), "{pill:?}");
    }
}

/// R14: a click on another cell whose commit `project.toml` refuses keeps
/// the open cell as it was: no warning for the rolled-back merge, no caret
/// placed, no sweep armed, and the click fails.
#[test]
fn a_refused_click_on_a_cell_leaves_the_open_one() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><CR>");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("b");
    std::fs::write(app.proj().root().join("project.toml"), "name = \n").unwrap();
    draw(&mut app);
    let r = app.hits.rect_of(&cell(2, 1)).unwrap();
    app.last_action_failed = false;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    let edit = app.project_tab.table.editing.as_ref().expect("the old cell stays open");
    let (row, _) = edit.cell().unwrap();
    assert_eq!((row, edit.input.text(), edit.input.cursor()), (0, "b", 1));
    assert!(app.text_drag.is_none());
    assert!(app.last_action_failed);
    let messages = app.toasts.messages();
    assert!(messages.iter().all(|m| !m.contains("duplicate")), "{messages:?}");
    assert!(messages.iter().any(|m| m.contains("cannot save default headers")), "{messages:?}");
}

/// `project.toml` with `n` default headers, `h0` to `h{n-1}`.
fn many(n: usize) -> String {
    let rows: String = (0..n).map(|i| format!("h{i} = \"{i}\"\n")).collect();
    format!("[default_headers]\n{rows}")
}

/// A wheel notch at (`col`, `row`), then a frame.
fn wheel_at(app: &mut App, (col, row): (u16, u16), down: bool) {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    app.handle_mouse(MouseEvent {
        kind: if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp },
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    });
    draw(app);
}

/// A wheel notch over the header table's first line.
fn wheel(app: &mut App, down: bool) {
    let r = app.project_tab.table_area;
    assert!(!r.is_empty(), "the table is drawn");
    wheel_at(app, (r.x + 1, r.y), down);
}

/// The wheel over the Project tab's header table scrolls it, so a row
/// below the bottom can be reached and clicked; the Environments and
/// Spaces list waiting behind the tab stays where it was. The cursor
/// stays, and its next move brings the view back to it.
#[test]
fn the_wheel_scrolls_the_header_table() {
    let mut app = project_app(&many(60), KeyProfile::Arrows);
    app.update(Action::SelectManageTab(ManageTab::Environments));
    draw(&mut app);
    app.update(Action::SelectManageTab(ManageTab::Project));
    draw(&mut app);
    assert!(app.hits.rect_of(&cell(59, 1)).is_none(), "the last row starts below the view");
    for _ in 0..30 {
        wheel(&mut app, true);
    }
    assert!(app.hits.rect_of(&cell(0, 1)).is_none(), "the first row scrolled off");
    assert_eq!(app.manage.list.scroll, 0, "the hidden list stays");
    assert!(app.project_tab.on_name(), "the cursor stays");
    click(&mut app, cell(59, 1));
    assert_eq!(
        app.field_caret(),
        crate::keyroute::FieldCaret::Live(FieldId::ProjectCell { row: 59, col: 1 }),
        "the clicked cell opened"
    );
    // The wheel back up never scrolls the open cell out of view.
    for _ in 0..5 {
        wheel(&mut app, false);
    }
    assert!(app.hits.rect_of(&cell(59, 1)).is_some(), "the open cell stays drawn");
}

/// The wheel anywhere else on the tab (over the Name row, the headings,
/// the space below the table) moves nothing, as the Variables tab's wheel
/// moves only what is under the pointer.
#[test]
fn the_wheel_off_the_table_moves_nothing() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(&many(60), profile);
        feed(&mut app, if profile == KeyProfile::Vim { "j" } else { "<Down>" });
        draw(&mut app);
        let name = app.hits.rect_of(&Hit::ProjectName).unwrap();
        let table = app.project_tab.table_area;
        for at in [(name.x + 1, name.y), (table.x, table.y - 2), (table.x, table.y - 1)] {
            wheel_at(&mut app, at, true);
        }
        let t = &app.project_tab.table;
        assert_eq!(t.scroll, 0, "{profile:?}");
        assert!(!t.scrolled_away(), "{profile:?}");
        assert_eq!(t.selected(), Some(Stop::Row(0)), "{profile:?}");
    }
    // Below a short table.
    let mut app = project_app(THREE, KeyProfile::Arrows);
    draw(&mut app);
    let table = app.project_tab.table_area;
    wheel_at(&mut app, (table.x, table.bottom() + 1), true);
    assert_eq!(app.project_tab.table.scroll, 0);
}

/// A wheel notch that moves nothing (the headers fit) marks nothing, as
/// on a request's table: the view still follows the cursor.
#[test]
fn a_wheel_that_moves_nothing_marks_nothing() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down>");
    draw(&mut app);
    wheel(&mut app, true);
    assert_eq!(app.project_tab.table.scroll, 0);
    assert!(!app.project_tab.table.scrolled_away());
}

/// In vim the wheel pulls the table's cursor into the view, as on a
/// request's table (spec 2026-10-25 S24); the arrows wheel leaves it.
#[test]
fn the_vim_wheel_pulls_the_cursor_into_view() {
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let mut app = project_app(&many(60), profile);
        feed(&mut app, if profile == KeyProfile::Vim { "j" } else { "<Down>" });
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)), "{profile:?}");
        draw(&mut app);
        for _ in 0..5 {
            wheel(&mut app, true);
        }
        let table = &app.project_tab.table;
        assert!(table.scroll > 1, "{profile:?}: the view scrolled");
        if profile == KeyProfile::Vim {
            // Row `r` is line `r + 1`: the first row in view.
            assert_eq!(table.selected(), Some(Stop::Row(table.scroll - 1)), "{profile:?}");
            assert!(!table.scrolled_away(), "{profile:?}");
        } else {
            assert_eq!(table.selected(), Some(Stop::Row(0)), "{profile:?}");
            assert!(table.scrolled_away(), "{profile:?}");
        }
    }
}

/// The keys scroll the table the least they can: ↓ past the bottom
/// moves the view one line, and ↑ inside the view moves none.
#[test]
fn the_view_follows_the_cursor_the_least_it_can() {
    let mut app = project_app(&many(60), KeyProfile::Arrows);
    feed(&mut app, "<Down>");
    let viewport = app.project_tab.table.viewport;
    assert!(viewport > 3 && viewport < 60, "{viewport}");
    // Row r is line r + 1; the last line in view is viewport - 1.
    for _ in 0..viewport - 2 {
        feed(&mut app, "<Down>");
    }
    assert_eq!(app.project_tab.table.scroll, 0, "the cursor's line is the last in view");
    feed(&mut app, "<Down>");
    assert_eq!(app.project_tab.table.scroll, 1, "one line past it scrolls one");
    feed(&mut app, "<Up><Up>");
    assert_eq!(app.project_tab.table.scroll, 1, "moves inside the view scroll none");
}

/// As on the Settings tab, a click away drops the tab's cursor, so the
/// Name well is no longer drawn lifted; a key brings it back.
#[test]
fn a_click_away_drops_the_cursor() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    assert!(app.project_tab.focused, "the tab coming up holds the cursor");
    let well = app.hits.rect_of(&Hit::ProjectName).unwrap();
    let lifted = |app: &mut App| {
        draw(app);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 48)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        terminal.backend().buffer()[(well.x + 1, well.y)].bg
    };
    let focused_bg = lifted(&mut app);
    click(&mut app, Hit::ManageTab(ManageTab::Project.index()));
    assert!(!app.project_tab.focused);
    assert_ne!(lifted(&mut app), focused_bg, "the well is no longer lifted");
    feed(&mut app, "<Down>");
    assert!(app.project_tab.focused);
}

/// An open name or cell offers the Copy/Paste menu on a right click.
#[test]
fn a_right_click_on_the_open_name_offers_copy_and_paste() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    click(&mut app, Hit::ProjectName);
    assert!(app.project_tab.name_open());
    let r = app.hits.rect_of(&Hit::ProjectName).unwrap();
    app.handle_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Right,
        ),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    let Some(crate::components::modal::Modal::Dropdown(d)) = app.modals.top() else {
        panic!("no menu")
    };
    let labels: Vec<&str> = d.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["Copy", "Paste"]);
    assert!(app.project_tab.name_open(), "the name stays open under its menu");
}

/// `THREE` with `a` taken out, as an edit outside the app would leave it.
const WITHOUT_A: &str = "name = \"Orders\"\n\n[default_headers]\nb = \"2\"\nc = \"3\"\n";

/// The Project tab with `b`'s value cell open, `typed` in it.
fn open_b_value(typed: &str) -> App {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><Down><Right><CR>");
    let edit = app.project_tab.table.editing.as_mut().expect("b's value is open");
    assert_eq!(edit.cell().unwrap().0, 1);
    edit.input.set_text(typed);
    app
}

/// R14, task #78: a reload of files changed on disk commits an open cell
/// before the re-read, so the typing lands in the header it was typed
/// against, not the one that took its row.
#[test]
fn a_reload_of_changed_files_commits_an_open_cell_first() {
    let mut app = open_b_value("22");
    std::fs::write(app.proj().root().join("project.toml"), WITHOUT_A).unwrap();
    assert!(app.update(Action::ReloadProjectFiles), "the files changed");
    assert!(app.project_tab.table.editing.is_none(), "the cell closed");
    let headers = &app.proj().meta().default_headers;
    assert_eq!(headers["b"].value, "22", "the typing is b's");
    assert_eq!(headers["c"].value, "3", "c untouched");
}

/// A cell whose commit is refused is cancelled by the reload, with the
/// toast the Variable Manager's gives.
#[test]
fn a_reload_cancels_a_project_cell_that_refuses_to_close() {
    let mut app = open_b_value("22");
    std::fs::write(app.proj().root().join("project.toml"), "name = \n").unwrap();
    app.update(Action::ReloadProjectFiles);
    assert!(app.project_tab.table.editing.is_none(), "the edit is cancelled");
    assert!(app.toasts.messages().contains(&EDIT_DISCARDED), "{:?}", app.toasts.messages());
    assert_eq!(file(&app), "name = \n", "nothing written");
}

/// Reload from disk commits an open cell first too, and stops on one that
/// refuses, the typing kept.
#[test]
fn reload_from_disk_commits_an_open_cell_or_stops() {
    let mut app = open_b_value("22");
    std::fs::write(app.proj().root().join("project.toml"), WITHOUT_A).unwrap();
    app.update(Action::ReloadFromDisk);
    assert!(app.project_tab.table.editing.is_none(), "the cell closed");
    assert_eq!(app.proj().meta().default_headers["b"].value, "22", "the typing is b's");

    let mut app = open_b_value("22");
    std::fs::write(app.proj().root().join("project.toml"), "name = \n").unwrap();
    app.last_action_failed = false;
    app.update(Action::ReloadFromDisk);
    assert!(app.last_action_failed, "the reload stopped");
    let edit = app.project_tab.table.editing.as_ref().expect("still open");
    assert_eq!(edit.input.text(), "22", "the typing kept");
}

/// A journaled op closes an open Project input first (`begin_op`), saved,
/// so the input's step comes before the op's on the undo stack.
#[test]
fn a_journaled_op_commits_an_open_project_input_first() {
    let mut app = open_b_value("22");
    app.update(Action::CreateEnv("staging".into()));
    assert!(app.project_tab.table.editing.is_none(), "the cell closed");
    assert_eq!(app.proj().meta().default_headers["b"].value, "22");
    feed(&mut app, "<C-z>");
    assert_ne!(app.active_env(), Some("staging"), "the first undo takes the environment");
    assert_eq!(app.proj().meta().default_headers["b"].value, "22", "and keeps the header");
    feed(&mut app, "<C-z>");
    assert_eq!(app.proj().meta().default_headers["b"].value, "2", "the second takes the header");

    // The name the same way.
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("Billing");
    app.update(Action::CreateEnv("staging".into()));
    assert!(!app.project_tab.name_open());
    assert_eq!(app.proj().meta().name.as_deref(), Some("Billing"));
}

/// The tab's own writes go through `begin_op` too, which leaves the cell
/// a Tab walk opened: Tab commits the name and the value opens.
#[test]
fn tab_commits_a_cell_and_opens_the_next() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><CR>");
    app.project_tab.table.editing.as_mut().unwrap().input.set_text("A2");
    feed(&mut app, "<Tab>");
    assert_eq!(keys(&app), ["A2", "b", "c"]);
    let edit = app.project_tab.table.editing.as_ref().expect("the value opened");
    assert_eq!(edit.cell().unwrap(), (0, crate::components::table_editor::Col::Value));
}

/// The footer's delete chip keeps the tab's cursor, which the chip's
/// click took away, on the row that moved up.
#[test]
fn the_footer_delete_chip_keeps_the_cursor() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down><Down>");
    click(&mut app, Hit::FooterChip(Action::DeleteDefaultHeader(1)));
    assert_eq!(keys(&app), ["a", "c"]);
    assert!(app.project_tab.focused, "the cursor is drawn");
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(1)));
}

/// Every footer chip the tab shows keeps its cursor drawn, as its keys
/// do: add, copy and delete on a row, and done on an open field.
#[test]
fn every_footer_chip_keeps_the_cursor() {
    for chip in ["add", "copy", "delete", "done"] {
        let mut app = project_app(THREE, KeyProfile::Arrows);
        feed(&mut app, if chip == "done" { "<CR>" } else { "<Down><Down>" });
        let chips = app.project_tab.footer_chips(3);
        let action = chips
            .iter()
            .find(|(_, l, _)| *l == chip)
            .and_then(|(_, _, a)| a.clone())
            .unwrap_or_else(|| panic!("no {chip} chip"));
        click(&mut app, Hit::FooterChip(action));
        assert!(app.project_tab.focused, "{chip}: the cursor is drawn");
    }
}

/// The verb keys light the cursor a click away dropped, as every key the
/// tab takes does: `y` copies and the row is drawn lit again.
#[test]
fn a_verb_key_after_a_click_away_lights_the_cursor() {
    for (profile, yank) in [(KeyProfile::Arrows, "y"), (KeyProfile::Vim, "yy")] {
        let mut app = project_app(THREE, profile);
        feed(&mut app, if profile == KeyProfile::Vim { "j" } else { "<Down>" });
        click(&mut app, Hit::ManageTab(ManageTab::Project.index()));
        assert!(!app.project_tab.focused, "{profile:?}");
        feed(&mut app, yank);
        assert_eq!(super::tests::last_copied(&app).as_deref(), Some("1"), "{profile:?}");
        assert!(app.project_tab.focused, "{profile:?}: the cursor is drawn");
    }
}

/// ctrl+PageDown is no move key: it is handed on from every stop, and the
/// cursor stays.
#[test]
fn a_chord_that_is_no_move_is_handed_on_from_every_stop() {
    for p in 0..=4 {
        let mut app = project_app(THREE, KeyProfile::Arrows);
        app.project_tab.table.set_selected(place_stop(p));
        let ev = KeyEvent::new(KeyCode::PageDown, KeyModifiers::CONTROL);
        assert_eq!(app.handle_project_key(ev), None, "{p}: handed on");
        assert_eq!(app.project_tab.table.selected(), place_stop(p), "{p}");
    }
}

/// The stop at place `p` of the tab's walk on `THREE`: the Name row (no
/// table cursor), the three headers, then `+ Add header`.
fn place_stop(p: usize) -> Option<Stop> {
    walk_place(3, p)
}

/// The stop at place `p` of the tab's walk over `n` headers: the Name row
/// (the table's head), the headers, then `+ Add header`.
fn walk_place(n: usize, p: usize) -> Option<Stop> {
    Some(match p {
        0 => Stop::Head(0),
        p if p <= n => Stop::Row(p - 1),
        _ => Stop::Add,
    })
}

/// The header table's view shows the cursor: from its top on the Name
/// row, which is drawn above it, and with the cursor's line in sight on a
/// stop of the table.
fn assert_view_shows_cursor(app: &App, n: usize, at: &str) {
    let t = &app.project_tab.table;
    match t.selected().and_then(|stop| stop.line(n)) {
        None => assert_eq!(t.scroll, 0, "{at}: the Name row shows the table's top"),
        Some(line) => {
            assert!(
                line >= t.scroll && line < t.scroll + t.viewport,
                "{at}: line {line} out of view {}+{}",
                t.scroll,
                t.viewport
            );
        }
    }
}

/// R12: every move key, from every stop, in both profiles, at rest and
/// out of an open name or cell, lands where the table's one walk puts it,
/// the Name row being the walk's first place, and the table's view
/// follows it there, to the table's top on the Name row. Over `n`
/// headers the Name row is place 0, the headers 1 to `n` and `+ Add
/// header` `n + 1`, the last; a half page is half the last place, rounded
/// up (2 of 4 over three headers, the Name row counted as one). Sixty
/// headers make a table taller than the view, walked from a sample of its
/// stops, each settled in view first.
#[test]
fn every_move_key_walks_the_name_row_and_the_table_as_one() {
    type Rule = fn(usize, usize) -> usize;
    let up: Rule = |p, _| p.saturating_sub(1);
    let down: Rule = |p, last| (p + 1).min(last);
    let half_up: Rule = |p, last| p.saturating_sub(last.div_ceil(2));
    let half_down: Rule = |p, last| (p + last.div_ceil(2)).min(last);
    let up2: Rule = |p, _| p.saturating_sub(2);
    let down2: Rule = |p, last| (p + 2).min(last);
    let first: Rule = |_, _| 0;
    let last: Rule = |_, last| last;
    let at_rest: &[(&str, Rule)] = &[
        ("<Up>", up),
        ("<Down>", down),
        ("<Home>", first),
        ("<PageUp>", first),
        ("<C-Home>", first),
        ("<C-b>", first),
        ("<End>", last),
        ("<PageDown>", last),
        ("<C-End>", last),
        ("<C-f>", last),
        ("<C-d>", half_down),
        ("<C-u>", half_up),
    ];
    let vim_at_rest: &[(&str, Rule)] = &[
        ("k", up),
        ("j", down),
        ("2k", up2),
        ("2j", down2),
        ("gg", first),
        ("G", last),
        ("3G", |_, _| 2),
    ];
    let out_of_open: &[(&str, Rule)] = &[("<Up>", up), ("<Down>", down)];
    let vim_out_of_open: &[(&str, Rule)] = &[("<Esc>k", up), ("<Esc>j", down)];
    let tall = many(60);
    let tables: [(&str, usize, &[usize]); 2] =
        [(THREE, 3, &[0, 1, 2, 3, 4]), (tall.as_str(), 60, &[0, 1, 2, 31, 60, 61])];
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let vim = profile == KeyProfile::Vim;
        let mut rest = at_rest.to_vec();
        let mut open = out_of_open.to_vec();
        if vim {
            rest.extend_from_slice(vim_at_rest);
            open.extend_from_slice(vim_out_of_open);
        }
        for &(toml, n, places) in &tables {
            let end = n + 1;
            let start = |p: usize| {
                let mut app = project_app(toml, profile);
                app.project_tab.table.set_selected(walk_place(n, p));
                draw(&mut app);
                app
            };
            for &p in places {
                for &(keys, rule) in &rest {
                    let at = format!("{profile:?} {n} from {p}: {keys}");
                    let mut app = start(p);
                    feed(&mut app, keys);
                    assert_eq!(
                        app.project_tab.table.selected(),
                        walk_place(n, rule(p, end)),
                        "{at}"
                    );
                    assert!(app.project_tab.field().is_none(), "{at}");
                    assert_view_shows_cursor(&app, n, &at);
                }
                for &(keys, rule) in &open {
                    let at = format!("{profile:?} {n} open at {p}: {keys}");
                    let mut app = start(p);
                    feed(&mut app, if vim { "i" } else { "<CR>" });
                    assert!(app.project_tab.field().is_some(), "{at}: opened");
                    feed(&mut app, keys);
                    assert_eq!(
                        app.project_tab.table.selected(),
                        walk_place(n, rule(p, end)),
                        "{at}"
                    );
                    assert!(app.project_tab.field().is_none(), "{at}");
                    assert_view_shows_cursor(&app, n, &at);
                }
            }
        }
    }
}

/// A wheel with the cursor on the Name row moves the table's view and
/// no cursor, in both profiles, since the Name row stays in sight; the
/// next move to the Name row brings the table back to its top.
#[test]
fn the_wheel_on_the_name_row_holds_the_view_until_a_move() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(&many(60), profile);
        assert!(app.project_tab.on_name(), "{profile:?}");
        for _ in 0..5 {
            wheel(&mut app, true);
        }
        assert!(app.project_tab.on_name(), "{profile:?}: the cursor stays");
        assert!(app.project_tab.table.scroll > 0, "{profile:?}: the view moved");
        feed(&mut app, if profile == KeyProfile::Vim { "gg" } else { "<Home>" });
        assert!(app.project_tab.on_name(), "{profile:?}");
        assert_eq!(app.project_tab.table.scroll, 0, "{profile:?}: back to the top");
    }
}

/// R12: ↑↓ walk the stops whether or not a field is open, committing it
/// first: ↑ from a cell open on the first row reaches the Name row as ↑
/// at rest does, and ↓ from the open name reaches the first header.
#[test]
fn the_arrows_walk_on_from_an_open_field() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(THREE, profile);
        feed(&mut app, if profile == KeyProfile::Vim { "ji" } else { "<Down><CR>" });
        let edit = app.project_tab.table.editing.as_mut().expect("a's name is open");
        assert_eq!(edit.cell().unwrap().0, 0, "{profile:?}");
        edit.input.set_text("A");
        feed(&mut app, "<Up>");
        assert_eq!(keys(&app), ["A", "b", "c"], "{profile:?}: committed");
        assert!(app.project_tab.on_name(), "{profile:?}");
        assert!(app.project_tab.table.editing.is_none(), "{profile:?}");

        let mut app = project_app(THREE, profile);
        feed(&mut app, if profile == KeyProfile::Vim { "i" } else { "<CR>" });
        app.project_tab.name_input_mut().expect("the name is open").set_text("Down");
        feed(&mut app, "<Down>");
        assert_eq!(app.proj().meta().name.as_deref(), Some("Down"), "{profile:?}");
        assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)), "{profile:?}");
        assert!(!app.project_tab.name_open(), "{profile:?}");
    }
    // ↑ from the open name commits it and stays: there is no stop above.
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("Up");
    feed(&mut app, "<Up>");
    assert_eq!(app.proj().meta().name.as_deref(), Some("Up"));
    assert!(app.project_tab.on_name());
    assert!(!app.project_tab.name_open());
}

/// R19: a body too small for the tab draws none of it, and nothing on it
/// acts at rest: no key or verb toggles, deletes, adds or opens what is
/// not on screen; Esc still leaves, and the footer offers only that.
#[test]
fn a_tab_too_small_to_draw_acts_on_nothing() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = project_app(THREE, profile);
        feed(&mut app, if profile == KeyProfile::Vim { "jj" } else { "<Down><Down>" });
        let before = file(&app);
        let mut terminal = Terminal::new(TestBackend::new(160, 12)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.project_tab.hidden, "{profile:?}");
        assert!(app.hits.rect_of(&Hit::ProjectName).is_none(), "{profile:?}: nothing drawn");
        let labels: Vec<&str> =
            app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
        assert_eq!(labels, ["back"], "{profile:?}");
        let presses = if profile == KeyProfile::Vim {
            ["<Space>", "dd", "a", "i", "yy", "G", "k"]
        } else {
            ["<Space>", "d", "a", "<CR>", "y", "<End>", "<Up>"]
        };
        for keys in presses {
            for ev in crate::keyroute::notation::events(keys) {
                app.handle_key(ev);
            }
            assert_eq!(file(&app), before, "{profile:?} {keys}: nothing written");
            assert!(app.project_tab.field().is_none(), "{profile:?} {keys}: nothing opened");
            assert_eq!(
                app.project_tab.table.selected(),
                Some(Stop::Row(1)),
                "{profile:?} {keys}: the cursor stays"
            );
        }
        assert_eq!(super::tests::last_copied(&app), None, "{profile:?}");
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Main, "{profile:?}: Esc leaves");
    }
}

/// Vim's `ZQ` and `:q!` leave the screen abandoning an open name, as they
/// abandon a Settings or Variables field: nothing is written.
#[test]
fn zq_abandons_an_open_name() {
    for q in ["ZQ", ":q!<CR>"] {
        let mut app = project_app(THREE, KeyProfile::Vim);
        let before = file(&app);
        feed(&mut app, "i<C-u>Gone<Esc>");
        assert!(app.project_tab.name_open(), "{q}");
        feed(&mut app, q);
        assert_eq!(app.screen, Screen::Main, "{q}");
        assert!(!app.project_tab.name_open(), "{q}: the edit abandoned");
        assert_eq!(file(&app), before, "{q}: nothing written");
    }
}

/// R4: the palette opens the tab.
#[test]
fn the_palette_opens_the_tab() {
    let commands = crate::components::palette::all_commands();
    let tab = Some(ManageTab::Project);
    assert!(commands.iter().any(|c| c.action == Action::OpenManage { tab }));
}

/// The footer advertises the tab's keys, and the way back.
#[test]
fn the_footer_names_the_keys() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    feed(&mut app, "<Down>");
    let chips = app.project_tab.footer_chips(3);
    let labels: Vec<&str> = chips.iter().map(|(_, l, _)| *l).collect();
    assert_eq!(labels, ["move", "edit", "add", "copy", "toggle", "delete", "back"]);
    feed(&mut app, "<Up>");
    let labels: Vec<&str> = app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
    assert_eq!(labels, ["move", "edit", "add", "back"], "the Name row adds, and has no row verbs");
}
