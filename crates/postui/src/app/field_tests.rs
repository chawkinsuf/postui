//! The vim profile's text fields at the app level (piece 4 spec §5.6; plan
//! M3a): sessions follow the caret, keys go through the engine, and what
//! the engine hands back never types.

use super::key_matrix_tests::{Fixture, build, draw, feed, snapshot, snapshot_diff};
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

/// Spec §3.7 and §8.1's undo check: a URL session is one app step, and one
/// undo brings back exactly the state before it.
#[test]
fn a_url_session_is_one_app_step() {
    let mut app = url_line("https://x.test/ab");
    let steps = app.history.undo_len();
    let before = snapshot(&app);
    feed(&mut app, "<CR>xx");
    assert_eq!(app.history.undo_len(), steps, "the session holds its keystrokes");
    feed(&mut app, "<Esc>");
    assert_eq!(app.history.undo_len(), steps + 1, "one step for the whole session");
    feed(&mut app, "u");
    let after = snapshot(&app);
    assert!(after == before, "undo did not restore exactly:\n{}", snapshot_diff(&before, &after));
}

#[test]
fn u_inside_a_session_steps_the_engine_not_the_app() {
    let mut app = url_line("https://x.test/ab");
    let steps = app.history.undo_len();
    feed(&mut app, "<CR>xxu");
    assert_eq!(url(&app).0, "https://x.test/a");
    assert_eq!(app.history.undo_len(), steps);
    assert!(app.field_caret().is_live());
}

/// Plan M3a Deviation 8.
#[test]
fn ctrl_z_and_ctrl_shift_z_are_u_and_ctrl_r_in_idle_normal() {
    let mut app = url_line("abc");
    feed(&mut app, "<CR>x<C-z>");
    assert_eq!(url(&app).0, "abc");
    assert!(app.field_caret().is_live(), "the field's own step; the field stays open");
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
    assert_eq!(url(&app).0, "ab");
}

/// Main's rule (step 1c): a field that has run out of undo steps but can
/// still redo owns the key, so `u` does not reach the app history then.
#[test]
fn the_field_owns_u_while_it_can_still_redo() {
    let mut app = vim(Fixture::TableAfterDelete);
    let cell = |app: &App| app.editor.table.editing.as_ref().map(|e| e.input.text().to_string());
    feed(&mut app, "l<CR>x");
    assert_eq!(cell(&app).as_deref(), Some(""));
    feed(&mut app, "u");
    assert_eq!(cell(&app).as_deref(), Some("2"), "the engine's own undo");
    feed(&mut app, "u");
    assert_eq!(app.editor.headers.len(), 2, "the app history is not touched");
    assert!(app.field_caret().is_live());
    feed(&mut app, "<Esc>u");
    assert_eq!(app.editor.headers.len(), 3, "closed, `u` is the app's again");
}

/// Review Focus 3 and plan M3a Deviation 5.
#[test]
fn an_app_undo_under_a_live_url_rebases_its_session() {
    let mut app = url_line("abc");
    app.history.break_coalescing();
    app.editor.url = LineInput::new("abcdef");
    app.capture_undo();
    feed(&mut app, "<CR>u");
    assert_eq!(url(&app).0, "abc", "nothing in the field to undo: the app's step came back");
    assert!(app.field_caret().is_live(), "the field stays open");
    assert!(!app.glue.can_undo(EngineKind::Line), "the session started over on the new text");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "ab", "the next x deletes under the caret on the new text");
}

/// Amendment C1: ctrl+z in Insert, before the engine has committed a step,
/// belongs to the field (its live edit counts), so the typed text is not
/// lost to an older app step.
#[test]
fn ctrl_z_in_insert_before_the_first_engine_step_leaves_the_app_history_alone() {
    let mut app = url_line("abc");
    let steps = app.history.undo_len();
    feed(&mut app, "<CR>Acd");
    assert_eq!(url(&app).0, "abccd");
    feed(&mut app, "<C-z>");
    assert_eq!(app.history.undo_len(), steps, "no app step was spent or applied");
    assert!(app.field_caret().is_live());
    assert_eq!(url(&app).0, "abccd", "the typed text is not lost to an older app step");
}

/// Review F1: an app op flushing an edited URL session must not wipe the
/// body's engine history, which outlives the body's own closes.
#[test]
fn a_flush_for_the_url_keeps_the_bodys_engine_history() {
    let mut app = vim(Fixture::UrlLine);
    app.editor.url = LineInput::new("abc");
    app.capture_undo();
    app.editor.active_tab = EditorTab::Body;
    app.editor.preferred_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    draw(&mut app);
    feed(&mut app, "lihello<Esc><Esc>");
    assert_eq!(app.editor.body.lines.to_string(), "hello");
    assert_eq!(app.glue.session(), FieldSession::None, "the body closed");
    app.editor.sub_focus = SubFocus::Url;
    draw(&mut app);
    feed(&mut app, "<CR>x");
    assert_eq!(url(&app).0, "ab");
    app.flush_field_session();
    feed(&mut app, "<Esc>");
    assert_eq!(url(&app).0, "ab");
    app.editor.sub_focus = SubFocus::Content;
    draw(&mut app);
    feed(&mut app, "lu");
    assert_eq!(app.editor.body.lines.to_string(), "", "the body's own last edit undone");
    assert_eq!(url(&app).0, "ab", "not the app's step");
}

/// Review F2: a flush mid-session. The text it took is the new start, and
/// the rest of the session is one more step.
#[test]
fn a_flush_mid_session_splits_it_into_two_exact_steps() {
    let mut app = url_line("abc");
    let start = snapshot(&app);
    feed(&mut app, "<CR>Axy");
    app.flush_field_session();
    app.capture_undo();
    let flushed = snapshot(&app);
    feed(&mut app, "z<Esc><Esc>");
    assert_eq!(url(&app).0, "abcxyz");
    feed(&mut app, "u");
    let after = snapshot(&app);
    assert!(after.same_but_field(&flushed), "first undo:\n{}", snapshot_diff(&flushed, &after));
    // App undo does not restore an open field or its mode (user ruling 2026-10-03).
    assert_eq!(app.field_caret(), FieldCaret::None);
    feed(&mut app, "u");
    let after = snapshot(&app);
    assert!(after == start, "second undo:\n{}", snapshot_diff(&start, &after));
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

const BODY: &str = "{\n  \"a\": 1\n}";

fn body(app: &App) -> String {
    app.editor.body_text()
}

fn caret(app: &App) -> (usize, usize) {
    (app.editor.body.cursor.row, app.editor.body.cursor.col)
}

#[test]
fn the_body_runs_the_engine_in_vim() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jdd");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    assert_eq!(body(&app), "{\n}");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY);
    feed(&mut app, "ggjA,<Esc>");
    assert_eq!(body(&app), "{\n  \"a\": 1,\n}");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
}

/// Spec §5.6: Esc in idle Normal is today's routing; the body blurs.
#[test]
fn esc_in_idle_normal_blurs_the_body() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "<Esc>");
    assert_eq!(app.editor.sub_focus, SubFocus::None);
    assert_eq!(app.glue.session(), FieldSession::None);
}

/// Spec §3.7 item 4 and plan M3a Deviation 6: edtui's own keys never run
/// in vim. ctrl+e (edtui's end of line) and ctrl+k (kill line) are bound
/// to nothing, in Normal and in Insert.
#[test]
fn edtui_never_runs_in_the_vim_body() {
    let mut app = vim(Fixture::Body);
    let before = (body(&app), caret(&app));
    feed(&mut app, "<C-e><C-k>");
    assert_eq!((body(&app), caret(&app)), before);
    feed(&mut app, "i<C-e><C-k><Esc>");
    assert_eq!((body(&app), caret(&app)), before);
}

/// Plan M3a Deviation 6: the body's alt+←/→ word jump is arrows-only; in
/// vim the keymap's chord runs.
#[test]
fn alt_arrows_are_the_keymaps_in_the_vim_body() {
    let mut app = vim(Fixture::Body);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_ne!(app.editor.active_tab, EditorTab::Body, "the tab cycle ran");
    assert_eq!(body(&app), BODY);
}

/// Spec §3.7: the body gets one app step per session.
#[test]
fn a_body_session_is_one_app_step() {
    let mut app = vim(Fixture::Body);
    let steps = app.history.undo_len();
    feed(&mut app, "xjx");
    assert_eq!(app.history.undo_len(), steps, "the session holds its edits");
    feed(&mut app, "<Esc>");
    assert_eq!(app.history.undo_len(), steps + 1);
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY);
}

/// Piece 3 §3.11: the body's state outlives a visit elsewhere.
#[test]
fn re_entering_the_body_keeps_its_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>");
    feed(&mut app, "jj");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body), "back in by keyboard");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step from the first visit");
}

/// Spec §3.7: a format with the body open is one engine step.
#[test]
fn a_format_with_the_body_open_is_one_engine_step() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("{\"a\":1}");
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
    assert_eq!(body(&app), postui_core::json::format("{\"a\":1}").unwrap());
    feed(&mut app, "u");
    assert_eq!(body(&app), "{\"a\":1}", "u takes the format back");
}

/// Piece 3 §4.4: the engine pages by the rows the body last drew.
#[test]
fn ctrl_d_pages_by_the_drawn_body() {
    let mut app = vim(Fixture::Body);
    let lines: Vec<String> = (0..100).map(|i| format!("  \"k{i}\": {i},")).collect();
    app.editor.set_body_text(&lines.join("\n"));
    draw(&mut app);
    feed(&mut app, "<C-d>");
    let half = usize::from(app.editor.last_body_area.unwrap().height) / 2;
    assert_eq!(caret(&app).0, half);
}

/// Plan M3a Deviation 19: the body's history never follows into another
/// request. (`ForceCreateRequest`'s `begin_op` also flushes the session;
/// the glue's own test `the_body_state_follows_the_open_request` pins the
/// slug rule alone, and Task 8's `a_request_switch_clears_the_search_highlight`
/// pins it in the app.)
#[test]
fn a_request_switch_drops_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x");
    assert!(app.glue.can_undo(EngineKind::Body));
    app.update(Action::ForceCreateRequest("other".into()));
    feed(&mut app, "<Esc>");
    assert!(!app.glue.can_undo(EngineKind::Body));
}

/// Amendment C2: the body's state follows the open request's slug when a
/// rename changes it in place. The edit is from an earlier visit, so the
/// rename's `begin_op` has no live session edits to flush.
#[test]
fn renaming_the_open_request_keeps_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    let from = app.editor.slug.clone().expect("the fixture's request is saved");
    app.update(Action::RenameRequest { from: from.clone(), to: "renamed".into() });
    assert_ne!(app.editor.slug.as_deref(), Some(from.as_str()), "the editor follows the new slug");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step, not the app's rename");
}

/// Amendment C2 (controller ruling 2): an undo of the rename re-keys the
/// open request in place too, and the body's state follows it back.
#[test]
fn undoing_a_rename_keeps_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    let from = app.editor.slug.clone().expect("the fixture's request is saved");
    app.update(Action::RenameRequest { from: from.clone(), to: "renamed".into() });
    feed(&mut app, "<Esc>");
    assert_eq!(app.glue.session(), FieldSession::None, "the body closed: `u` is the app's");
    feed(&mut app, "u");
    assert_eq!(app.editor.slug.as_deref(), Some(from.as_str()), "the rename was undone in place");
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    draw(&mut app);
    let steps = app.history.undo_len();
    feed(&mut app, "u");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    assert_eq!(body(&app), BODY, "the engine's step from the earlier visit");
    assert_eq!(app.history.undo_len(), steps, "not the app's step for the same edit");
}

/// Amendment C2: a scratch saved under a name keeps the body's state.
#[test]
fn saving_a_scratch_keeps_the_bodys_history() {
    let mut app = App::new_for_test();
    assert_eq!(app.editor.slug, None, "a scratch");
    app.editor.method = postui_core::model::Method::Post;
    app.editor.set_body_text(BODY);
    app.editor.active_tab = EditorTab::Body;
    app.editor.preferred_tab = EditorTab::Body;
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    app.capture_undo();
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body));
    app.update(Action::SaveRequestAs("saved".into()));
    assert!(app.editor.slug.is_some(), "the scratch is saved");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step from before the save");
}

/// Amendment C3: a carry out of the body lends the real body to the
/// engine, so leaving it strips an unused autoindent there, and its
/// history survives for the next visit.
#[test]
fn a_carry_out_of_the_body_leaves_the_real_body() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jo");
    assert_eq!(body(&app), "{\n  \"a\": 1\n  \n}", "o autoindents");
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    let at = |kind| MouseEvent { kind, column: area.x, row: area.y, modifiers: KeyModifiers::NONE };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Url), "the click carried the session");
    let text = body(&app);
    assert!(!text.split('\n').any(|l| !l.is_empty() && l.trim().is_empty()), "a whitespace-only line is left: {text:?}");
    feed(&mut app, "<Esc><Esc>j");
    assert_eq!(app.glue.session(), FieldSession::Live(FieldId::Body), "back in by keyboard");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the body's history survived the carry");
}

/// Review Focus 4: the arrows body still goes through edtui.
#[test]
fn the_arrows_body_still_types_through_edtui() {
    let mut app = build(Fixture::Body);
    draw(&mut app);
    feed(&mut app, "x");
    assert_eq!(body(&app), format!("x{BODY}"), "a letter types");
    feed(&mut app, "<C-e>");
    assert_eq!(caret(&app), (0, 2), "edtui's end of line");
    assert_eq!(app.glue.session(), FieldSession::None);
}

/// Plan M3a Deviation 13: Vim's bracketed paste in Normal.
#[test]
fn a_paste_in_normal_goes_after_the_caret_as_one_step() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>");
    app.paste_text("X\nY");
    assert_eq!(url(&app), ("abX Y", 4), "flattened, after the caret, the caret on the last char");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "ab");
}

#[test]
fn a_paste_at_column_zero_goes_before_the_caret() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>0");
    app.paste_text("XY");
    assert_eq!(url(&app), ("XYab", 1));
}

#[test]
fn a_paste_in_insert_is_part_of_the_insert() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>A");
    app.paste_text("cd");
    assert_eq!(url(&app), ("abcd", 4));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    feed(&mut app, "<Esc>u");
    assert_eq!(url(&app).0, "ab", "one undo step with the insert");
}

#[test]
fn a_body_paste_keeps_its_line_breaks() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "j$");
    app.paste_text("x\ny");
    assert_eq!(body(&app), "{\n  \"a\": 1x\ny\n}");
}

#[test]
fn a_paste_into_the_search_prompt_types_the_pattern() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "/");
    app.paste_text("\"a");
    feed(&mut app, "<CR>");
    assert_eq!(caret(&app), (1, 2), "the search ran with the pasted pattern");
}

/// Spec §3.7: a picked token is an edit outside the key path.
#[test]
fn a_picked_token_goes_in_at_the_insert_caret() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>A");
    app.update(Action::InsertVarText("{{v}}".into()));
    assert_eq!(url(&app), ("ab{{v}}", 7));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
}

#[test]
fn a_picked_token_in_normal_goes_after_the_caret() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>");
    app.update(Action::InsertVarText("{{v}}".into()));
    assert_eq!(url(&app).0, "ab{{v}}");
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "ab", "one engine step");
}

/// Main's `{{` trigger, through the engine (Task 4's `after_typed`).
#[test]
fn typing_two_braces_opens_the_variable_picker() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>A{{");
    assert!(matches!(app.modals.top(), Some(Modal::VarPicker(_))));
    assert_eq!(app.glue.session(), FieldSession::Parked(FieldId::Url));
    assert_eq!(url(&app).0, "ab{{");
}

use ratatui::style::Modifier;

fn draw_buffer(app: &mut App) -> ratatui::buffer::Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn row_text(buf: &ratatui::buffer::Buffer, area: ratatui::layout::Rect, y: u16) -> String {
    (area.x..area.right()).map(|x| buf[(x, y)].symbol()).collect()
}

/// The modifiers of each cell on the body's first row that shows `c`.
fn cells_of(buf: &ratatui::buffer::Buffer, area: ratatui::layout::Rect, c: &str) -> Vec<Modifier> {
    (area.x..area.right()).filter(|&x| buf[(x, area.y)].symbol() == c).map(|x| buf[(x, area.y)].modifier).collect()
}

/// Spec §5.6: the prompt is drawn on the body pane's bottom row.
#[test]
fn the_search_prompt_draws_on_the_bodys_bottom_row() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("foo bar foo");
    feed(&mut app, "/fo");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let bottom = row_text(&buf, area, area.bottom() - 1);
    assert!(bottom.starts_with("/fo"), "{bottom:?}");
    feed(&mut app, "<Esc>");
    let buf = draw_buffer(&mut app);
    assert!(!row_text(&buf, area, area.bottom() - 1).starts_with("/fo"), "Esc closes the prompt");
}

/// Plan M3a Deviation 16: matches REVERSED, the one Enter lands on also BOLD.
#[test]
fn search_matches_paint_and_the_landing_one_is_bold() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("foo bar foo");
    feed(&mut app, "/foo");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let fs = cells_of(&buf, area, "f");
    assert_eq!(fs.len(), 2, "both f's are drawn");
    assert!(fs[1].contains(Modifier::REVERSED | Modifier::BOLD), "incsearch: where Enter lands");
    assert!(!cells_of(&buf, area, "b")[0].contains(Modifier::REVERSED), "bar is no match");
    feed(&mut app, "<CR>");
    let buf = draw_buffer(&mut app);
    let fs = cells_of(&buf, area, "f");
    assert!(fs.iter().all(|m| m.contains(Modifier::REVERSED)), "the highlight stays after the search");
    assert!(fs.iter().all(|m| !m.contains(Modifier::BOLD)), "no prompt, no landing match");
}

/// Spec §3.14: a request switch clears the highlight (plan M3a Deviation 19).
/// `ForceCreateRequest`: the dirty Body fixture would raise the unsaved
/// changes confirm for `CreateRequest` (amendment A8-1).
#[test]
fn a_request_switch_clears_the_search_highlight() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("x foo");
    feed(&mut app, "/foo<CR>");
    app.update(Action::ForceCreateRequest("other".into()));
    app.editor.method = postui_core::model::Method::Post;
    app.editor.set_body_text("x foo");
    app.editor.active_tab = EditorTab::Body;
    app.focus = PaneId::Editor;
    app.editor.sub_focus = SubFocus::Content;
    feed(&mut app, "0");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    assert!(!cells_of(&buf, area, "f")[0].contains(Modifier::REVERSED));
}

/// Amendment C2 follow-on: a rename changes the slug in place, so the
/// highlight stays.
#[test]
fn renaming_the_open_request_keeps_the_search_highlight() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("x foo");
    feed(&mut app, "/foo<CR>");
    let from = app.editor.slug.clone().expect("the fixture's request is saved");
    app.update(Action::RenameRequest { from, to: "renamed".into() });
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    // The `o`: the caret's own cell on the `f` is REVERSED regardless.
    assert!(cells_of(&buf, area, "o")[0].contains(Modifier::REVERSED), "the highlight survives the rename");
}

/// Amendment A8-2: a match below the viewport is computed for the frame a
/// jump scrolls it into view.
#[test]
fn a_far_search_paints_its_matches_in_the_frame_it_scrolls() {
    let mut app = vim(Fixture::Body);
    let mut lines: Vec<String> = (0..120).map(|i| format!("row{i}")).collect();
    lines[100] = "needle".into();
    app.editor.set_body_text(&lines.join("\n"));
    draw(&mut app);
    feed(&mut app, "/needle<CR>");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let hit = (area.y..area.bottom())
        .find(|&y| row_text(&buf, area, y).contains("needle"))
        .expect("the match scrolled into view");
    // The second letter: the caret's own cell is REVERSED regardless.
    let x = (area.x..area.right()).find(|&x| buf[(x, hit)].symbol() == "e").unwrap();
    assert!(buf[(x, hit)].modifier.contains(Modifier::REVERSED), "painted in the frame that scrolled");
}
