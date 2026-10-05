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
