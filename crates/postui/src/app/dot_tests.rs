//! `.` at the app level (key list §6 `L.dot`, spec §5.12; plan M5b Task 4).

use super::key_matrix_tests::{Fixture, build, draw, feed, snapshot, snapshot_diff};
use super::*;
use crate::components::manage::ManageTab;
use crate::components::sidebar::Row;
use crate::config::KeyProfile;

fn vim(f: Fixture) -> App {
    let mut app = build(f);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    app
}

fn requests(app: &App) -> Vec<&str> {
    app.sidebar
        .rows
        .iter()
        .filter_map(|r| match r {
            Row::Request { slug, .. } => Some(slug.as_str()),
            Row::Folder { .. } => None,
        })
        .collect()
}

fn headers(app: &App) -> Vec<&str> {
    app.editor.headers.keys().map(String::as_str).collect()
}

fn envs(app: &App) -> Vec<String> {
    app.manage_items(ManageTab::Environments).to_vec()
}

/// Probes D1 and D5: `.` does the last `dd` or `p` again.
#[test]
fn dot_repeats_the_last_list_change() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "dd.");
    assert_eq!(requests(&app), ["main/alpha"]);
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "dd.");
    assert_eq!(headers(&app), ["a"]);
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "yyp.");
    assert_eq!(requests(&app).len(), 5, "two copies: {:?}", requests(&app));
}

/// Review Focus 2 (probes D3, D11): a count replaces the one `.` had and
/// is kept for the next.
#[test]
fn a_new_count_replaces_the_old() {
    let mut app = vim(Fixture::TableThreeHeaders);
    for (k, v) in [("d", "4"), ("e", "5"), ("f", "6"), ("g", "7"), ("h", "8")] {
        app.editor.headers.insert(k.into(), postui_core::model::Entry { value: v.into(), enabled: true });
    }
    app.editor.table.selected = Some(0);
    app.capture_undo();
    feed(&mut app, "2dd");
    assert_eq!(headers(&app), ["c", "d", "e", "f", "g", "h"]);
    feed(&mut app, "3.");
    assert_eq!(headers(&app), ["f", "g", "h"]);
    feed(&mut app, ".");
    assert!(headers(&app).is_empty(), "the 3 stuck: {:?}", headers(&app));
}

/// Review Focus 2 (probes D4, D14): undo and redo leave `.` alone.
#[test]
fn dot_survives_undo_and_redo() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "ddu");
    assert_eq!(requests(&app), ["main/alpha", "main/beta", "main/gamma"]);
    feed(&mut app, ".");
    assert_eq!(requests(&app), ["main/alpha", "main/gamma"], "the undone delete again");
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "ddu<C-r>.");
    assert_eq!(requests(&app).len(), 1, "{:?}", requests(&app));
}

/// Probes D7 and D9: a yank is no change, and `:delete` leaves `.` the
/// `dd` it had, as Vim's command-line commands do (Deviation 6).
#[test]
fn dot_survives_yanks_and_the_command_line() {
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "ddyy.");
    assert_eq!(headers(&app), ["a"]);
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "dd:delete<CR>");
    assert_eq!(requests(&app), ["main/alpha"]);
    feed(&mut app, ".");
    assert!(requests(&app).is_empty(), "the `dd` again: {:?}", requests(&app));
}

/// Review Focus 1 (Deviation 6): a rename, a cell's edit, or a line run
/// over an open cell is the last change, which a list cannot repeat:
/// `.` does nothing.
#[test]
fn a_change_elsewhere_makes_dot_stale() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "ddr<Esc>ccdelta<CR>");
    assert_eq!(requests(&app), ["main/alpha", "main/delta"], "gamma renamed");
    feed(&mut app, ".");
    assert_eq!(requests(&app), ["main/alpha", "main/delta"], "the rename was the last change");
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "ddix<Esc><Esc>");
    assert!(!app.field_caret().is_live(), "the cell closed");
    feed(&mut app, ".");
    assert_eq!(app.editor.headers.len(), 2, "the cell's edit was the last change");
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "ddix<Esc>:w<CR>.");
    assert_eq!(app.editor.headers.len(), 2, "the cell closed with the line: its edit may be the last change");
    // The body never closes, but its engine session holds the Insert's
    // edit until a palette command flushes it (`:e!` discards through
    // `flush_field_session`): that edit is the last change too.
    let mut app = build(Fixture::Body);
    let get = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/1""#).unwrap();
    for slug in ["main/alpha", "main/beta"] {
        postui_core::fixtures::save_request(app.proj().root(), slug, &get).unwrap();
    }
    app.update(Action::RefreshSidebar);
    // Persisted, so `:e!`'s reload keeps the profile (no keymap question).
    super::key_matrix_tests::use_persisted_profile(&mut app, KeyProfile::Vim);
    app.focus = PaneId::Sidebar;
    app.sidebar.selected = app.sidebar.rows.iter().position(|r| matches!(r, Row::Request { slug, .. } if slug == "main/alpha"));
    draw(&mut app);
    feed(&mut app, "dd");
    assert_eq!(requests(&app), ["main/beta", "main/matrix"]);
    feed(&mut app, "<C-w>l");
    assert_eq!(app.surface(), crate::keyroute::Surface::Body, "back in the body");
    feed(&mut app, "ix<Esc>");
    assert!(app.field_gate(), "the body's session holds the edit");
    feed(&mut app, ":e!<CR><C-w>h");
    assert_eq!(app.surface(), crate::keyroute::Surface::Sidebar);
    assert!(app.toasts.messages().contains(&"Changes discarded — u undoes"), "the `:e!` ran: {:?}", app.toasts.messages());
    feed(&mut app, ".");
    assert_eq!(requests(&app), ["main/beta", "main/matrix"], "the body's edit was the last change");
}

/// Probe D0: nothing to repeat, nothing said.
#[test]
fn dot_with_nothing_to_repeat_is_silent() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ".");
    assert_eq!(requests(&app), ["main/alpha", "main/beta", "main/gamma"]);
    assert!(app.toasts.messages().is_empty(), "{:?}", app.toasts.messages());
}

/// Deviation 2 (the user's decision): `.` runs wherever the change runs;
/// where it has no row it does nothing and the change stays.
#[test]
fn dot_runs_where_the_change_runs() {
    let mut app = vim(Fixture::ManageEnvs);
    feed(&mut app, "dd<C-w>l.");
    assert_eq!(envs(&app), ["qa", "dev"], "no `dd` on the detail pane");
    feed(&mut app, "<C-w>h.");
    assert_eq!(envs(&app), ["dev"], "back on the list, it deletes");
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "dd<C-w>h.");
    assert!(requests(&app).is_empty(), "the table's `dd`, repeated on the sidebar: {:?}", requests(&app));
    assert!(app.modals.is_empty(), "delete never confirms");
}

/// Spec §5.12 (the user's rule): each repeat is its own undo step.
#[test]
fn each_repeat_is_its_own_undo_step() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "dd.u");
    assert_eq!(requests(&app), ["main/alpha", "main/gamma"]);
}

/// Amendment A4.3 (the user's rule, undo restores exactly): `u` after a
/// `.` brings back the whole app as it was before the `.`, files, rows,
/// cursor, open request and field, and undoes only the `.`'s step.
#[test]
fn undo_after_dot_restores_exactly() {
    for (fixture, keys) in [(Fixture::SidebarThree, "dd"), (Fixture::SidebarThree, "yyp"), (Fixture::TableThreeHeaders, "yyP")] {
        let mut app = vim(fixture);
        feed(&mut app, keys);
        let before = snapshot(&app);
        feed(&mut app, ".");
        assert_ne!(snapshot(&app), before, "{fixture:?} {keys}.: the `.` changed something");
        feed(&mut app, "u");
        let after = snapshot(&app);
        assert!(after == before, "{fixture:?} {keys}.u did not restore exactly:\n{}", snapshot_diff(&before, &after));
    }
}
