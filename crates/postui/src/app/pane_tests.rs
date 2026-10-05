//! Panes and tabs at the app level (piece 4 spec §5.8; plan M4a), and the
//! plan's Review Focus cases.

use super::key_matrix_tests::{Fixture, build, draw, feed};
use super::*;
use crate::config::KeyProfile;
use crate::keyroute::FieldId;
use crate::keyroute::engine_glue::FieldSession;

fn app_in(f: Fixture, profile: KeyProfile) -> App {
    let mut app = build(f);
    app.use_profile(profile);
    draw(&mut app);
    app
}

/// Review Focus 3: a count gt to a disabled tab moves nothing, caret included.
#[test]
fn a_refused_count_gt_moves_nothing() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    assert!(app.editor.body_tab_disabled(), "the fixture is a GET");
    assert_eq!(app.editor.active_tab, EditorTab::Headers);
    feed(&mut app, "4gt");
    assert_eq!(app.editor.active_tab, EditorTab::Headers, "Body is disabled");
    assert_eq!(app.editor.sub_focus, SubFocus::Url, "the caret stays");
    feed(&mut app, "9gt");
    assert_eq!(app.editor.active_tab, EditorTab::Headers, "out of range");
    feed(&mut app, "2gt");
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    assert_eq!(app.editor.sub_focus, SubFocus::Content, "a tab it selected lands the caret");
}

/// Amendment C1 (the user's decision): a refused `{N}gt` typed in an open
/// field leaves the field open, its text and caret as they were; one that
/// selects a tab closes the field first.
#[test]
fn a_refused_count_gt_in_an_open_field_keeps_it_open() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    app.editor.url = LineInput::new("https://x.test/ab");
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, "<CR>");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "0");
    let before = (app.editor.url.text().to_string(), app.editor.url.cursor());
    for keys in ["4gt", "9gt"] {
        feed(&mut app, keys);
        assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url), "{keys}: the field stays open");
        assert!(app.editor.url_open(), "{keys}");
        assert_eq!(app.mode_indicator(), Some("NORMAL"), "{keys}");
        assert_eq!((app.editor.url.text().to_string(), app.editor.url.cursor()), before, "{keys}: text and caret");
        assert_eq!(app.editor.active_tab, EditorTab::Headers, "{keys}: the tab stays");
        assert_eq!(app.key_echo(), "", "{keys}");
    }
    feed(&mut app, "2gt");
    assert!(!app.field_caret().is_live(), "a tab it selects closes the field");
    assert_eq!(app.editor.url.text(), "https://x.test/ab", "the text is kept");
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
}

/// Final review M-1: a tab step that lands on the tab already showing
/// changes nothing, as in vim: the table keeps its row.
#[test]
fn a_tab_step_onto_the_current_tab_keeps_the_table_row() {
    let mut app = app_in(Fixture::TableThreeHeaders, KeyProfile::Vim);
    feed(&mut app, "j");
    assert_eq!(app.editor.table.selected, Some(2));
    // Headers is tab 1 of the GET's three; 12 and 3 steps back go round.
    for keys in ["1gt", "12gT", "3gT"] {
        feed(&mut app, keys);
        assert_eq!(app.editor.active_tab, EditorTab::Headers, "{keys}");
        assert_eq!(app.editor.table.selected, Some(2), "{keys}: the row stays");
        assert_eq!(app.editor.sub_focus, SubFocus::Content, "{keys}");
        assert_eq!(app.key_echo(), "", "{keys}");
    }
}

/// Final review M-1: a `{N}gt` onto the body's own tab leaves the body's
/// session whole, so `u` still undoes the engine's last edit, not the
/// whole visit as one app step.
#[test]
fn a_count_gt_onto_the_body_tab_keeps_the_engine_history() {
    let mut app = app_in(Fixture::Body, KeyProfile::Vim);
    feed(&mut app, "x");
    let after_first = app.editor.body_text();
    feed(&mut app, "jx");
    let steps = app.history.undo_len();
    // A POST: Headers, Params, Vars, Body.
    feed(&mut app, "4gt");
    assert_eq!(app.editor.active_tab, EditorTab::Body);
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    assert_eq!(app.history.undo_len(), steps, "the session still holds its edits");
    feed(&mut app, "u");
    assert_eq!(app.editor.body_text(), after_first, "u undoes the engine's last edit only");
}

/// The user's decision (2026-10-05, as amendment C1 for `{N}gt`): a
/// `<C-w>` direction with no pane there, typed in an open field, leaves
/// the field open, its text and caret as they were; one that moves closes
/// the field first.
#[test]
fn a_refused_ctrl_w_in_an_open_field_keeps_it_open() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    app.editor.url = LineInput::new("https://x.test/ab");
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, "<CR>");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    feed(&mut app, "0");
    let before = (app.editor.url.text().to_string(), app.editor.url.cursor());
    // The editor is the top right pane: nothing above it, nothing right.
    for keys in ["<C-w>k", "<C-w>l"] {
        feed(&mut app, keys);
        assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url), "{keys}: the field stays open");
        assert!(app.editor.url_open(), "{keys}");
        assert_eq!(app.mode_indicator(), Some("NORMAL"), "{keys}");
        assert_eq!((app.editor.url.text().to_string(), app.editor.url.cursor()), before, "{keys}: text and caret");
        assert_eq!(app.focus, PaneId::Editor, "{keys}: focus stays");
        assert_eq!(app.key_echo(), "", "{keys}");
    }
    feed(&mut app, "<C-w>j");
    assert!(!app.field_caret().is_live(), "a move closes the field");
    assert_eq!(app.editor.url.text(), "https://x.test/ab", "the text is kept");
    assert_eq!(app.focus, PaneId::Response);
    // A Manage screen's detail pane: `<C-w>l` has nowhere to go.
    let mut app = app_in(Fixture::VarForm, KeyProfile::Vim);
    feed(&mut app, "i<Esc><C-w>l");
    assert!(app.field_caret().is_live(), "a refused move on Manage keeps the field open too");
    assert_eq!(app.varmanager.focus, crate::components::varmanager::VmFocus::Form);
}

/// Final review M-3 (the user's decision, 2026-10-05): vim binds no
/// alt+←/→, and on the main and Manage screens the chord does nothing; it
/// never reaches a list as a plain arrow.
#[test]
fn vim_alt_arrows_do_nothing_on_the_response() {
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    // A one-liner far wider than the pane, so a plain → would scroll it.
    let body = "x".repeat(300);
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![],
            sent_headers: vec![],
            body: body.clone(),
            ttfb: std::time::Duration::from_millis(1),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: None,
        })),
        app.session.send_generation,
    );
    draw(&mut app);
    let mode = app.session.response.view().map(|v| v.mode);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    draw(&mut app);
    assert_eq!(app.session.response.view().unwrap().h_scroll, 0, "no sideways scroll");
    assert_eq!(app.session.response.view().map(|v| v.mode), mode, "no tab step");
    // A plain → does scroll it, so the check above can fail.
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(app.session.response.view().unwrap().h_scroll > 0, "the fixture scrolls");
    // The arrows profile's chord still steps the strip.
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Arrows);
    let mode = app.session.response.view().map(|v| v.mode);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_ne!(app.session.response.view().map(|v| v.mode), mode, "arrows cycles the tabs");
}

/// Final review M-3: as above, on the sidebar (a folder a plain → opens)
/// and the Variables list (a plain → enters the detail pane).
#[test]
fn vim_alt_arrows_do_nothing_on_the_sidebar_and_manage() {
    use crate::components::sidebar::Row;
    use crate::components::varmanager::VmFocus;
    let mut app = app_in(Fixture::SidebarThree, KeyProfile::Vim);
    let req = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/1""#).unwrap();
    postui_core::fixtures::save_request(app.proj().root(), "main/f/inner", &req).unwrap();
    app.update(Action::RefreshSidebar);
    draw(&mut app);
    let folder = app.sidebar.rows.iter().position(|r| matches!(r, Row::Folder { .. })).expect("the folder is listed");
    app.sidebar.selected = Some(folder);
    // Collapsed, so a plain → would open it.
    if matches!(app.sidebar.rows[folder], Row::Folder { expanded: true, .. }) {
        app.update(Action::ToggleSelectedFolder);
    }
    let rows = |app: &App| app.sidebar.rows.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>();
    let before = (rows(&app), app.sidebar.selected, app.editor.active_tab);
    for code in [KeyCode::Right, KeyCode::Left] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::ALT));
        draw(&mut app);
        assert_eq!((rows(&app), app.sidebar.selected, app.editor.active_tab), before, "{code:?}");
    }
    let mut app = app_in(Fixture::VarList, KeyProfile::Vim);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.varmanager.focus, VmFocus::List, "the list keeps the keyboard");
    assert_eq!(app.manage.tab, crate::components::manage::ManageTab::Variables);
}

/// Spec §5.8: `{N}gt` on the response's strip as drawn, and on the Manage
/// screen's; a tab that is not there does nothing.
#[test]
fn a_count_gt_selects_on_the_response_and_manage_strips() {
    use crate::components::manage::ManageTab;
    use crate::components::response::ViewMode;
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    feed(&mut app, "3gt");
    assert_eq!(app.session.response.view().map(|v| v.mode), Some(ViewMode::Headers));
    feed(&mut app, "4gt");
    assert_eq!(app.session.response.view().map(|v| v.mode), Some(ViewMode::Headers), "three tabs");
    assert_eq!(app.focus, PaneId::Response);
    let mut app = app_in(Fixture::ManageEnvs, KeyProfile::Vim);
    feed(&mut app, "3gt");
    assert_eq!(app.manage.tab, ManageTab::Spaces);
    feed(&mut app, "5gt");
    assert_eq!(app.manage.tab, ManageTab::Spaces, "four tabs");
}

/// Review Focus 1: `g` is a field prefix now, and `gg` still reaches the
/// body's engine.
#[test]
fn gg_in_the_body_still_reaches_the_engine() {
    let mut app = app_in(Fixture::Body, KeyProfile::Vim);
    feed(&mut app, "G");
    assert_eq!(app.editor.body.cursor.row, 2);
    feed(&mut app, "gg");
    assert_eq!(app.editor.body.cursor.row, 0);
    assert_eq!(app.key_echo(), "");
}

/// Spec §5.8: the sidebar borrows the editor's strip; focus never moves.
#[test]
fn gt_on_the_sidebar_steps_the_editor_strip_from_where_it_stands() {
    let mut app = app_in(Fixture::SidebarThree, KeyProfile::Vim);
    let before = app.editor.active_tab;
    feed(&mut app, "gt");
    assert_ne!(app.editor.active_tab, before);
    assert_eq!(app.focus, PaneId::Sidebar);
}

/// Plan M4a Deviation 7: arrows' alt+→ keeps the caret where it is.
#[test]
fn arrows_tab_chord_keeps_the_caret() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Arrows);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
}

/// Plan M4a Deviation 8: a cycle on a GET steps over the disabled Body tab
/// by any distance.
#[test]
fn a_multi_step_cycle_skips_the_disabled_body_tab() {
    let mut app = app_in(Fixture::TableThreeHeaders, KeyProfile::Vim);
    feed(&mut app, "gT");
    assert_eq!(app.editor.active_tab, EditorTab::Vars, "Headers back one, over Body");
    feed(&mut app, "5gT");
    assert_eq!(app.editor.active_tab, EditorTab::Headers, "Vars back five over three enabled tabs");
}

/// Review Focus 5: a focus change made without ctrl+w is learned.
#[test]
fn ctrl_w_l_returns_to_the_right_pane_last_used() {
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    // A click's focus change, outside any key.
    app.update(Action::FocusPane(PaneId::Editor));
    feed(&mut app, "<C-w>h");
    assert_eq!(app.focus, PaneId::Sidebar);
    feed(&mut app, "<C-w>l");
    assert_eq!(app.focus, PaneId::Editor, "the right pane last used, not the response");
    feed(&mut app, "<C-w>j<C-w>p");
    assert_eq!(app.focus, PaneId::Editor, "p goes back to the pane before");
    feed(&mut app, "<C-w>p");
    assert_eq!(app.focus, PaneId::Response, "and p again returns");
}

/// Review Focus 5: the right pane is learned from where the keyboard was,
/// not the default (the editor) and not only from `ctrl+w`'s own moves.
#[test]
fn ctrl_w_l_learns_the_right_pane_from_a_plain_key() {
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    feed(&mut app, "j");
    assert_eq!(app.focus, PaneId::Response);
    // A focus change outside any key.
    app.update(Action::FocusPane(PaneId::Sidebar));
    feed(&mut app, "<C-w>l");
    assert_eq!(app.focus, PaneId::Response, "the response was the right pane last used");
}

/// A left press on the sidebar pane's background, through `handle_mouse`.
fn click_sidebar(app: &mut App) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    draw(app);
    let r = app.hits.rect_of(&crate::hit::Hit::Pane(PaneId::Sidebar)).expect("the sidebar pane is drawn");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x + 1,
        row: r.y + r.height.saturating_sub(2),
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.focus, PaneId::Sidebar, "the click focused the sidebar");
}

/// Review Focus 5 (fix round 1): a click with no key before it is learned
/// too: the memory notes the pane at the start of every input event.
#[test]
fn ctrl_w_learns_the_pane_a_click_left() {
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    assert_eq!(app.focus, PaneId::Response);
    click_sidebar(&mut app);
    feed(&mut app, "<C-w>l");
    assert_eq!(app.focus, PaneId::Response, "l: the right pane the click left");
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    click_sidebar(&mut app);
    feed(&mut app, "<C-w>p");
    assert_eq!(app.focus, PaneId::Response, "p: the pane before the click");
}

/// Review Focus 5: nothing earlier, nothing happens.
#[test]
fn ctrl_w_p_with_no_earlier_pane_does_nothing() {
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Vim);
    feed(&mut app, "<C-w>p");
    assert_eq!(app.focus, PaneId::Response);
    assert!(app.toasts.messages().is_empty());
}

/// Review Focus 2: `<C-w>` then a key that is no window command, in an
/// open field, edits nothing; the next key is fresh.
#[test]
fn ctrl_w_then_junk_in_a_field_edits_nothing() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    app.editor.url = crate::components::line_input::LineInput::new("abc");
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, "i<Esc><C-w>x");
    assert_eq!(app.editor.url.text(), "abc");
    assert!(app.field_caret().is_live(), "the field stays open");
    feed(&mut app, "x");
    assert_eq!(app.editor.url.text(), "ab", "the next x is fresh");
}

/// Spec §5.8 and plan M4a Deviation 5: list and detail on the Manage screens.
#[test]
fn ctrl_w_moves_between_a_manage_list_and_its_detail() {
    use crate::components::manage_list::ListFocus;
    use crate::components::varmanager::VmFocus;
    let mut app = app_in(Fixture::ManageEnvs, KeyProfile::Vim);
    feed(&mut app, "<C-w>l");
    assert_eq!(app.manage.list.focus, ListFocus::Detail);
    feed(&mut app, "<C-w>j");
    assert_eq!(app.manage.list.focus, ListFocus::Detail, "no pane below");
    feed(&mut app, "<C-w>w");
    assert_eq!(app.manage.list.focus, ListFocus::List, "w: the other half");
    let mut app = app_in(Fixture::VarGrid, KeyProfile::Vim);
    feed(&mut app, "l<C-w>h");
    assert_eq!(app.varmanager.focus, VmFocus::List, "from any grid column");
    feed(&mut app, "<C-w>l");
    assert_eq!(app.varmanager.focus, VmFocus::Grid);
    let mut app = app_in(Fixture::VarForm, KeyProfile::Vim);
    feed(&mut app, "i<Esc><C-w>h");
    assert_eq!(app.varmanager.focus, VmFocus::List, "an open form field closes first");
    assert!(!app.field_caret().is_live());
}
