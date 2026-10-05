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

/// Spec §5.8 as amended: a step onto the tab already showing changes
/// nothing, so typed in an open field it leaves the field open, caret and
/// all, as a refused `{N}gt` does.
#[test]
fn a_tab_step_onto_the_current_tab_keeps_an_open_field_open() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    app.editor.url = LineInput::new("https://x.test/ab");
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, "<CR>0");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    let before = (app.editor.url.text().to_string(), app.editor.url.cursor());
    // Headers is tab 1 of the GET's three.
    for keys in ["1gt", "3gT", "12gT"] {
        feed(&mut app, keys);
        assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url), "{keys}: the field stays open");
        assert!(app.editor.url_open(), "{keys}");
        assert_eq!((app.editor.url.text().to_string(), app.editor.url.cursor()), before, "{keys}: text and caret");
        assert_eq!(app.editor.active_tab, EditorTab::Headers, "{keys}");
        assert_eq!(app.editor.sub_focus, SubFocus::Url, "{keys}");
    }
}

/// As above, with the URL line selected but not open: the caret stays on
/// it, since no tab moved for it to land in.
#[test]
fn a_tab_step_onto_the_current_tab_keeps_the_url_line_selected() {
    let mut app = app_in(Fixture::UrlLine, KeyProfile::Vim);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    for keys in ["1gt", "3gT", "12gT"] {
        feed(&mut app, keys);
        assert_eq!(app.editor.active_tab, EditorTab::Headers, "{keys}");
        assert_eq!(app.editor.sub_focus, SubFocus::Url, "{keys}: the caret stays");
    }
    feed(&mut app, "gt");
    assert_eq!(app.editor.active_tab, EditorTab::Params);
    assert_eq!(app.editor.sub_focus, SubFocus::Content, "a step that moves lands the caret");
}

/// A field whose close is refused (a Settings value that will not parse)
/// keeps the keyboard: `{N}gt` and `gt` leave the tab where it is rather
/// than strand the live edit behind another tab.
#[test]
fn a_tab_key_whose_field_close_is_refused_moves_nothing() {
    use crate::components::manage::ManageTab;
    use crate::components::settings::SettingsField;
    for keys in ["2gt", "gt", "gT"] {
        let mut app = app_in(Fixture::Settings, KeyProfile::Vim);
        app.settings.begin_edit(SettingsField::Osc52Limit, "abc");
        draw(&mut app);
        feed(&mut app, "<Esc>");
        assert_eq!(app.settings.editing, Some(SettingsField::Osc52Limit), "the fixture's commit is refused");
        assert_eq!(app.mode_indicator(), Some("NORMAL"));
        feed(&mut app, keys);
        assert_eq!(app.manage.tab, ManageTab::Settings, "{keys}: the tab stays");
        assert_eq!(app.settings.editing, Some(SettingsField::Osc52Limit), "{keys}: the edit stays live");
        assert_eq!(app.settings.field_text(), "abc", "{keys}");
    }
}

/// A Settings field that does close lets the tab key go, even with a
/// Variables edit left live behind another tab (a failed click-away
/// commit leaves one, `screen_field` says): that edit is not this tab's.
#[test]
fn a_tab_key_leaves_a_settings_field_past_a_variables_edit_behind_it() {
    use crate::components::manage::ManageTab;
    use crate::components::settings::SettingsField;
    let mut app = app_in(Fixture::VarForm, KeyProfile::Vim);
    feed(&mut app, "i<Esc>");
    assert!(app.varmanager.form.editing.is_some(), "the Variables edit is live");
    app.manage.tab = ManageTab::Settings;
    app.settings.begin_edit(SettingsField::Osc52Limit, "1000");
    draw(&mut app);
    assert_eq!(app.field_caret(), crate::keyroute::FieldCaret::Live(FieldId::Settings(SettingsField::Osc52Limit)));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "gT");
    assert_eq!(app.settings.editing, None, "the Settings field closed");
    assert_ne!(app.manage.tab, ManageTab::Settings, "and the tab moved");
    assert!(app.varmanager.form.editing.is_some(), "the Variables edit is untouched");
}

/// A pending key the footer echoes is dropped by a key that then does
/// nothing (a refused window or tab key, vim's unbound alt+→): the event
/// still redraws, or the footer keeps showing it.
#[test]
fn a_refused_key_that_drops_the_echo_redraws() {
    let last = |keys: &str| *crate::keyroute::notation::events(keys).last().expect("a key");
    let cases: [(Fixture, &str, KeyEvent); 3] = [
        (Fixture::SidebarThree, "<C-w>", last("h")),
        (Fixture::UrlLine, "5g", last("t")),
        (Fixture::SidebarThree, "g", KeyEvent::new(KeyCode::Right, KeyModifiers::ALT)),
    ];
    for (fixture, prefix, ev) in cases {
        let mut app = app_in(fixture, KeyProfile::Vim);
        feed(&mut app, prefix);
        assert_ne!(app.key_echo(), "", "{prefix}: echoed");
        assert!(app.handle_key(ev), "{prefix} {ev:?}: redraws");
        assert_eq!(app.key_echo(), "", "{prefix} {ev:?}");
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

/// A `<C-w>` move whose field refuses to close (the variable it edits is
/// gone, so the commit fails and the edit stays live) moves nothing.
#[test]
fn a_ctrl_w_whose_field_close_is_refused_moves_nothing() {
    use crate::components::varmanager::VmFocus;
    let mut app = app_in(Fixture::VarForm, KeyProfile::Vim);
    feed(&mut app, "ix<Esc>");
    assert!(app.field_caret().is_live());
    app.proj_mut()
        .apply_var_edit(&postui_core::project::VarEdit::Delete { name: "alpha".into() })
        .unwrap();
    feed(&mut app, "<C-w>h");
    assert!(app.field_caret().is_live(), "the edit stays live");
    assert!(app.varmanager.form.editing.is_some());
    assert_eq!(app.varmanager.focus, VmFocus::Form, "the keyboard stays");
    assert!(!app.toasts.is_empty(), "the refusal says so");
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

/// The tab chord on the response walks Pretty, Raw, Headers and wraps both
/// ways, skipping Pretty when there is no tree to show (the strip
/// `App::tab_target` resolves for every tab key).
#[test]
fn the_tab_chord_walks_the_response_strip_both_ways() {
    use crate::components::response::ViewMode;
    let mut app = app_in(Fixture::ResponseLines, KeyProfile::Arrows);
    let mode = |app: &App| app.session.response.view().map(|v| v.mode);
    assert_eq!(mode(&app), Some(ViewMode::Pretty));
    assert!(app.update(Action::CycleTabs(-1)));
    assert_eq!(mode(&app), Some(ViewMode::Headers), "wraps backward");
    assert!(app.update(Action::CycleTabs(1)));
    assert_eq!(mode(&app), Some(ViewMode::Pretty), "wraps forward");
    assert!(app.update(Action::CycleTabs(1)));
    assert_eq!(mode(&app), Some(ViewMode::Raw));
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![],
            sent_headers: vec![],
            body: "not json".into(),
            ttfb: std::time::Duration::from_millis(1),
            elapsed: std::time::Duration::from_millis(1),
            size: 8,
            content_type: None,
        })),
        app.session.send_generation,
    );
    assert_eq!(mode(&app), Some(ViewMode::Raw));
    assert!(app.update(Action::CycleTabs(-1)));
    assert_eq!(mode(&app), Some(ViewMode::Headers), "no Pretty to land on");
    assert!(app.update(Action::CycleTabs(1)));
    assert_eq!(mode(&app), Some(ViewMode::Raw));
    app.session.response = Default::default();
    assert!(!app.update(Action::CycleTabs(1)), "no ready view: nothing to cycle");
}

/// The tab chord on the Manage screen walks its strip, Settings included,
/// and wraps both ways.
#[test]
fn the_tab_chord_walks_the_manage_strip_both_ways() {
    use crate::components::manage::ManageTab;
    let mut app = app_in(Fixture::ManageSpaces, KeyProfile::Arrows);
    assert_eq!(app.manage.tab, ManageTab::Spaces);
    assert!(app.update(Action::CycleTabs(1)));
    assert_eq!(app.manage.tab, ManageTab::Settings);
    assert!(app.update(Action::CycleTabs(1)));
    assert_eq!(app.manage.tab, ManageTab::Variables, "wraps forward");
    assert!(app.update(Action::CycleTabs(-1)));
    assert_eq!(app.manage.tab, ManageTab::Settings, "wraps backward");
}

/// Arrows' tab chord on the Manage screen closes the tab's field first, as
/// a click on the strip commits it: the typed value is saved, never dropped
/// or left live behind the next tab, and a close the field refuses keeps
/// the tab.
#[test]
fn arrows_tab_chord_commits_a_manage_field_first() {
    use crate::components::manage::ManageTab;
    use crate::components::settings::SettingsField;
    let alt_right = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
    let mut app = app_in(Fixture::Settings, KeyProfile::Arrows);
    app.ui_settings.osc52_limit = 65536;
    app.settings.begin_edit(SettingsField::Osc52Limit, "abc");
    app.handle_key(alt_right);
    assert_eq!(app.manage.tab, ManageTab::Settings, "a refused commit keeps the tab");
    assert_eq!(app.settings.editing, Some(SettingsField::Osc52Limit));
    assert_eq!(app.settings.field_text(), "abc");
    app.settings.begin_edit(SettingsField::Osc52Limit, "1024");
    app.handle_key(alt_right);
    assert_eq!(app.ui_settings.osc52_limit, 1024, "the value is committed, not dropped");
    assert_eq!(app.settings.editing, None);
    assert_ne!(app.manage.tab, ManageTab::Settings);

    let mut app = app_in(Fixture::VarForm, KeyProfile::Arrows);
    feed(&mut app, "<CR>x");
    assert!(app.varmanager.form.editing.is_some(), "the form field is open");
    let steps = app.history.undo_len();
    app.handle_key(alt_right);
    assert!(app.varmanager.form.editing.is_none(), "not left live behind the next tab");
    assert_eq!(app.history.undo_len(), steps + 1, "the edit is committed");
    assert_eq!(app.manage.tab, ManageTab::Environments);

    let mut app = app_in(Fixture::VarForm, KeyProfile::Arrows);
    feed(&mut app, "<CR>x");
    app.proj_mut()
        .apply_var_edit(&postui_core::project::VarEdit::Delete { name: "alpha".into() })
        .unwrap();
    app.handle_key(alt_right);
    assert!(app.varmanager.form.editing.is_some(), "a failed commit keeps the edit live");
    assert_eq!(app.manage.tab, ManageTab::Variables, "and the tab");
}

/// A Variables edit left live behind another tab (a failed click-away
/// commit) is not the open field there: `CloseField` on the Settings tab
/// does not reach that tab's Esc (which would close the screen).
#[test]
fn a_variables_edit_behind_another_tab_is_not_its_open_field() {
    use crate::components::manage::ManageTab;
    let mut app = app_in(Fixture::VarForm, KeyProfile::Arrows);
    feed(&mut app, "<CR>x");
    assert!(app.field_open());
    app.manage.tab = ManageTab::Settings;
    assert!(!app.field_open());
    assert!(app.open_text_field_mut().is_none());
    assert!(!app.update(Action::CloseField));
    assert_eq!(app.screen, Screen::Manage, "the screen stays up");
    assert!(app.varmanager.form.editing.is_some(), "the edit is untouched");
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

/// Code review: a disabled tab left showing steps from where it sits, so
/// one step lands on its next enabled neighbour either way.
#[test]
fn a_step_from_a_disabled_tab_lands_on_its_neighbour() {
    use crate::action::TabTo;
    let strip = || (vec![Some(Action::EditorTabSelect(0)), Some(Action::EditorTabSelect(1)), None, Some(Action::EditorTabSelect(3))], 2);
    assert_eq!(super::panes::strip_target(strip(), TabTo::By(1)), Some(Action::EditorTabSelect(3)));
    assert_eq!(super::panes::strip_target(strip(), TabTo::By(-1)), Some(Action::EditorTabSelect(1)));
    assert_eq!(super::panes::strip_target(strip(), TabTo::By(2)), Some(Action::EditorTabSelect(0)), "wraps");
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
