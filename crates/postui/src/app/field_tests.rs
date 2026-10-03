//! The vim profile's text fields at the app level (piece 4 spec §5.6; plan
//! M3a): sessions follow the caret, keys go through the engine, and what
//! the engine hands back never types.

use super::key_matrix_tests::{Fixture, build, draw, feed};
use super::*;
use crate::config::KeyProfile;
use crate::keyroute::engine_glue::FieldSession;
use crate::keyroute::{EngineKind, FieldCaret, FieldId};

fn vim(f: Fixture) -> App {
    let mut app = build(f);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    app
}

/// The URL line selected (not open) holding `text`.
fn url_line(text: &str) -> App {
    let mut app = vim(Fixture::UrlLine);
    app.editor.url = LineInput::new(text);
    app.capture_undo();
    draw(&mut app);
    app
}

fn url(app: &App) -> (&str, usize) {
    (app.editor.url.text(), app.editor.url.cursor())
}

fn header(app: &App, key: &str) -> Option<String> {
    app.editor.headers.get(key).map(|e| e.value.clone())
}

#[test]
fn enter_opens_the_url_in_normal_and_x_deletes_under_the_caret() {
    let mut app = url_line("https://x.test/ab");
    feed(&mut app, "<CR>");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert_eq!(url(&app), ("https://x.test/ab", 16), "a Normal caret rests on the last char");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "https://x.test/a");
    feed(&mut app, "<Esc>");
    assert!(!app.field_caret().is_live(), "Esc in idle Normal closes the field");
    assert_eq!(app.editor.sub_focus, SubFocus::Url, "the line stays selected");
    assert_eq!(url(&app).0, "https://x.test/a", "the text is kept");
    assert_eq!(app.glue.session(), FieldSession::None);
}

/// Spec §5.6 and plan M3a Deviation 12.
#[test]
fn the_url_opening_keys_seat_insert() {
    for (keys, want) in [("i", " Z ab"), ("a", "  Zab"), ("I", "  Zab"), ("A", "  abZ")] {
        let mut app = url_line("  ab");
        app.editor.url.set_cursor(1);
        feed(&mut app, keys);
        assert_eq!(app.mode_indicator(), Some("INSERT"), "{keys}");
        feed(&mut app, "Z");
        assert_eq!(url(&app).0, want, "{keys}");
    }
}

#[test]
fn insert_types_and_esc_steps_back_to_normal_then_closes() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>Acd");
    assert_eq!(url(&app), ("abcd", 4));
    feed(&mut app, "<Esc>");
    assert_eq!((app.mode_indicator(), url(&app).1), (Some("NORMAL"), 3), "Esc steps the caret back");
    assert!(app.field_caret().is_live(), "the first Esc only leaves Insert");
    feed(&mut app, "<Esc>");
    assert!(!app.field_caret().is_live());
}

/// Review Focus 2; plan M3a Deviations 6 and 9.
#[test]
fn declined_keys_never_type_or_edit() {
    for (keys, toast) in [
        ("qa", Some("Macros are not supported")),
        ("@a", Some("Macros are not supported")),
        ("ma", Some("Marks are not supported")),
        ("'a", Some("Marks are not supported")),
        ("`a", Some("Marks are not supported")),
        ("Zx", None),
        ("<C-w>", None),
        ("<C-e>", None),
    ] {
        let mut app = url_line("one two");
        feed(&mut app, "<CR>");
        app.toasts = Default::default();
        feed(&mut app, keys);
        assert_eq!(url(&app).0, "one two", "{keys}");
        assert_eq!(app.mode_indicator(), Some("NORMAL"), "{keys}");
        let toasts = app.toasts.messages();
        match toast {
            Some(t) => assert!(toasts.iter().any(|m| m.contains(t)), "{keys}: {toasts:?}"),
            None => assert!(toasts.is_empty(), "{keys}: {toasts:?}"),
        }
    }
}

/// Spec §5.6 (the user's decision 2).
#[test]
fn j_leaves_the_url_for_the_table() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>j");
    assert!(!app.field_caret().is_live());
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
}

#[test]
fn j_in_an_open_cell_commits_it_and_moves_down() {
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "l<CR>A9<Esc>j");
    assert_eq!(header(&app, "b").as_deref(), Some("29"));
    assert_eq!(app.editor.table.selected, Some(2));
    assert!(!app.field_caret().is_live());
}

/// Spec §5.6: `a` in an open cell is vim's append (it reverses vim-mode).
#[test]
fn a_in_an_open_cell_appends() {
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "l<CR>a9<Esc><Esc>");
    assert_eq!(header(&app, "b").as_deref(), Some("29"));
    assert_eq!(app.editor.headers.len(), 3, "no row was added");
}

/// Review Focus 1: Tab commits the cell the caret leaves, then carries.
#[test]
fn tab_carries_insert_to_the_next_cell() {
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "a");
    assert_eq!(app.mode_indicator(), Some("INSERT"), "an add verb opens in Insert");
    feed(&mut app, "X-A<Tab>");
    assert_eq!(app.mode_indicator(), Some("INSERT"), "the mode carries");
    assert!(matches!(app.glue.session(), FieldSession::Live(FieldId::TableCell { col: 1, .. })));
    feed(&mut app, "1<Esc><Esc>");
    assert_eq!(header(&app, "X-A").as_deref(), Some("1"));
}

/// Review Focus 4: the arrows profile does not change.
#[test]
fn the_arrows_profile_still_types_modelessly() {
    let mut app = build(Fixture::UrlLine);
    app.editor.url = LineInput::new("ab");
    draw(&mut app);
    feed(&mut app, "<CR>xq");
    assert_eq!(url(&app).0, "abxq", "letters type");
    assert_eq!(app.glue.session(), FieldSession::None);
    assert_eq!(app.mode_indicator(), None);
    feed(&mut app, "<C-z>");
    assert_eq!(url(&app).0, "ab", "the field's own undo");
}

/// Spec §3.3's park edge: a palette over an open field leaves it alone.
#[test]
fn a_palette_over_an_open_field_parks_it() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>:");
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    assert_eq!(app.glue.session(), FieldSession::Parked(FieldId::Url));
    feed(&mut app, "<Esc>");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
}

/// Plan M3a Deviations 12 and 17.
#[test]
fn a_settings_field_opens_in_insert_with_i() {
    use crate::components::settings::{SettingsField, SettingsRow, SettingsTab};
    let mut app = vim(Fixture::Settings);
    let row = SettingsTab::rows().iter().position(|r| *r == SettingsRow::Setting(SettingsField::AiCmd)).unwrap();
    app.settings.set_cursor(row);
    feed(&mut app, "i");
    assert!(matches!(app.glue.session(), FieldSession::Live(FieldId::Settings(_))));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    let seed = app.settings.field_text().to_string();
    feed(&mut app, "zz");
    assert_eq!(app.settings.field_text(), format!("{seed}zz"), "typing appends: arrows' select-all is gone");
}

#[test]
fn a_variable_form_field_edits_and_commits() {
    let mut app = vim(Fixture::VarForm);
    feed(&mut app, "id<Esc><Esc>");
    assert!(!app.field_caret().is_live());
    let vars = std::fs::read_to_string(app.proj().root().join("variables.toml")).unwrap();
    assert!(vars.contains("description = \"d\""), "{vars}");
}

#[test]
fn a_grid_cell_edits_and_commits() {
    let mut app = vim(Fixture::VarGrid);
    feed(&mut app, "<CR>A2<Esc><Esc>");
    assert!(!app.field_caret().is_live());
    let qa = std::fs::read_to_string(app.proj().root().join("environments/qa.toml")).unwrap();
    assert!(qa.contains("alice2"), "{qa}");
}

/// Spec §5.6: a click opens in the kind's start mode, caret where clicked.
#[test]
fn a_click_opens_the_url_in_normal_where_clicked() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = url_line("https://x.test/ab");
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    let at = |kind| MouseEvent { kind, column: area.x + 3, row: area.y, modifiers: KeyModifiers::NONE };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert_eq!(url(&app).1, 3);
}

/// Review Focus 5 and spec §4.3.
#[test]
fn switching_profiles_with_a_field_open_keeps_its_text() {
    use super::fields::Cause;
    let mut app = url_line("abc");
    feed(&mut app, "<CR>");
    app.use_profile(KeyProfile::Arrows);
    app.sync_session(None, Cause::Before);
    assert_eq!(app.glue.session(), FieldSession::None);
    assert!(app.field_caret().is_live(), "the field stays open");
    assert_eq!(url(&app), ("abc", 2), "the text and caret stay");
    app.use_profile(KeyProfile::Vim);
    app.sync_session(None, Cause::Before);
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("INSERT"), "arrows to vim enters Insert");
    assert_eq!(url(&app), ("abc", 2));
}

/// Spec §4.3 ("the caret untouched"): leaving Insert for the switch must
/// not step the caret back off the end of the line.
#[test]
fn switching_to_arrows_in_insert_keeps_the_caret_at_the_end() {
    use super::fields::Cause;
    let mut app = url_line("abc");
    feed(&mut app, "<CR>A");
    assert_eq!((app.mode_indicator(), url(&app)), (Some("INSERT"), ("abc", 3)));
    app.use_profile(KeyProfile::Arrows);
    app.sync_session(None, Cause::Before);
    assert_eq!(url(&app), ("abc", 3), "the caret stays past the last char");
    feed(&mut app, "d");
    assert_eq!(url(&app).0, "abcd", "arrows types where the caret was");
}

/// Spec §4.3 and "nothing requires vim": a switch to arrows made after the
/// body's vim session closed still gives edtui its Insert back, so the
/// arrows body types.
#[test]
fn the_body_types_in_arrows_after_a_vim_session_closed() {
    let mut app = vim(Fixture::TableThreeHeaders);
    app.editor.active_tab = EditorTab::Body;
    app.editor.preferred_tab = EditorTab::Body;
    draw(&mut app);
    feed(&mut app, "l");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    app.focus = PaneId::Sidebar;
    feed(&mut app, "j");
    assert_eq!(app.glue.session(), FieldSession::None, "the body's session closed");
    assert_eq!(app.editor.body.mode, edtui::EditorMode::Normal, "vim left edtui in Normal");
    app.use_profile(KeyProfile::Arrows);
    feed(&mut app, "<Tab>");
    assert_eq!(app.focus, PaneId::Editor, "the body is focused by keyboard");
    assert_eq!(app.field_caret(), FieldCaret::Live(FieldId::Body));
    let before = app.editor.body.lines.to_string();
    feed(&mut app, "x");
    assert_eq!(app.editor.body.lines.to_string(), format!("x{before}"), "the arrows body types");
}

/// Plan M3a Deviation 5: text replaced under a live session.
#[test]
fn text_replaced_under_a_session_rebases_it() {
    let mut app = url_line("abc");
    feed(&mut app, "<CR>x");
    assert!(app.glue.can_undo(EngineKind::Line));
    app.editor.url = LineInput::new("wxyz");
    feed(&mut app, "0");
    assert!(!app.glue.can_undo(EngineKind::Line), "the old history is gone");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "xyz", "the next key edits the new text");
}

/// Spec §3.8 and plan M3a Deviation 14.
#[test]
fn the_mode_word_follows_the_engine() {
    let mut app = url_line("abc");
    for (keys, word) in [
        ("<CR>", "NORMAL"),
        ("v", "VISUAL"),
        ("<Esc>V", "V-LINE"),
        ("<Esc>R", "REPLACE"),
        ("<Esc>i<C-o>", "(insert)"),
    ] {
        feed(&mut app, keys);
        assert_eq!(app.mode_indicator(), Some(word), "{keys}");
    }
}

/// Spec §3.8: the router's pending keys first, then the engine's.
#[test]
fn the_echo_shows_the_engines_half_typed_command() {
    let mut app = url_line("abc");
    feed(&mut app, "<CR>2");
    assert_eq!(app.key_echo(), "2", "the router holds the count");
    feed(&mut app, "d");
    assert_eq!(app.key_echo(), "2d", "the engine holds it now");
    feed(&mut app, "<Esc>");
    assert_eq!(app.key_echo(), "");
}

/// Spec §3.7: `u` with nothing to undo in the field goes to the app history.
#[test]
fn u_with_nothing_to_undo_in_the_field_reaches_the_app_history() {
    let mut app = vim(Fixture::TableAfterDelete);
    assert_eq!(app.editor.headers.len(), 2);
    feed(&mut app, "<CR>u");
    assert_eq!(app.editor.headers.len(), 3, "the row delete came back");
}

/// Plan M3a Deviation 15.
#[test]
fn an_engine_note_is_an_info_toast() {
    let mut app = url_line("abc");
    feed(&mut app, "<CR>");
    app.toasts = Default::default();
    feed(&mut app, "U");
    assert!(app.toasts.messages().iter().any(|m| m.contains("not supported")), "{:?}", app.toasts.messages());
}
