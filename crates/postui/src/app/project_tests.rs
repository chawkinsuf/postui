//! The Manage screen's Project tab (spec 2026-11-15 project settings):
//! the name and the default headers, written to `project.toml` at once
//! as one undo step each, in both key profiles.

use super::key_matrix_tests::{draw, feed, use_persisted_profile};
use super::*;
use crate::components::manage::ManageTab;
use crate::components::project_tab::ProjectStop;
use crate::components::table_editor::Stop;
use crate::config::KeyProfile;
use crate::keyroute::FieldId;

/// An app on a project whose `project.toml` is `toml`, the Project tab
/// open in `profile`.
pub(super) fn project_app(toml: &str, profile: KeyProfile) -> App {
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

/// `hit` as the Project table registers it.
fn pt(hit: Hit) -> Hit {
    Hit::ProjectTable(Box::new(hit))
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
        feed(&mut app, undo_key(profile));
        assert_eq!(file(&app), before, "{profile:?} {del}");
    }
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

/// R12: ↑ off the first row and Home go back to the Name row; Esc with
/// nothing open leaves the screen.
#[test]
fn the_cursor_walks_between_the_name_and_the_table() {
    let mut app = project_app(THREE, KeyProfile::Arrows);
    assert_eq!(app.project_tab.stop, ProjectStop::Name);
    feed(&mut app, "<Down>");
    assert_eq!(app.project_tab.stop, ProjectStop::Table);
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Row(0)));
    feed(&mut app, "<Up>");
    assert_eq!(app.project_tab.stop, ProjectStop::Name);
    feed(&mut app, "<End>");
    assert_eq!(app.project_tab.table.selected(), Some(Stop::Add));
    feed(&mut app, "<Home>");
    assert_eq!(app.project_tab.stop, ProjectStop::Name);
    feed(&mut app, "<Esc>");
    assert_eq!(app.screen, Screen::Main);
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
    // The name is refused the same way.
    app.project_tab.table.editing = None;
    app.project_tab.to_name();
    feed(&mut app, "<CR>");
    app.project_tab.name_input_mut().unwrap().set_text("New");
    feed(&mut app, "<CR>");
    assert_eq!(app.project_tab.name_input_mut().map(|i| i.text().to_string()), Some("New".into()));
}

fn click(app: &mut App, hit: Hit) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    draw(app);
    let r = app.hits.rect_of(&hit).unwrap_or_else(|| panic!("no {hit:?}"));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    app.capture_undo();
    draw(app);
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
    assert_eq!(tip(&app, pt(Hit::TableCopy(1))).as_deref(), Some("Copy this row's value"));
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
    assert_eq!((edit.row, edit.input.text(), edit.input.cursor()), (0, "b", 1));
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

fn wheel(app: &mut App, at: Hit, down: bool) {
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let r = app.hits.rect_of(&at).unwrap_or_else(|| panic!("no {at:?}"));
    app.handle_mouse(MouseEvent {
        kind: if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp },
        column: r.x + 1,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    });
    draw(app);
}

/// The wheel over the Project tab scrolls its header table, so a row
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
        wheel(&mut app, Hit::ProjectName, true);
    }
    assert!(app.hits.rect_of(&cell(0, 1)).is_none(), "the first row scrolled off");
    assert_eq!(app.manage.list.scroll, 0, "the hidden list stays");
    assert_eq!(app.project_tab.stop, ProjectStop::Name, "the cursor stays");
    click(&mut app, cell(59, 1));
    assert_eq!(
        app.field_caret(),
        crate::keyroute::FieldCaret::Live(FieldId::ProjectCell { row: 59, col: 1 }),
        "the clicked cell opened"
    );
    // The wheel back up never scrolls the open cell out of view.
    for _ in 0..5 {
        wheel(&mut app, Hit::ProjectName, false);
    }
    assert!(app.hits.rect_of(&cell(59, 1)).is_some(), "the open cell stays drawn");
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
    assert_eq!(edit.row, 1);
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
    assert_eq!((edit.row, edit.col), (0, crate::components::table_editor::Col::Value));
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
    assert_eq!(labels, ["move", "edit", "add", "toggle", "delete", "back"]);
    feed(&mut app, "<Up>");
    let labels: Vec<&str> = app.project_tab.footer_chips(3).iter().map(|(_, l, _)| *l).collect();
    assert_eq!(labels, ["move", "edit", "back"], "the Name row has no row verbs");
}
