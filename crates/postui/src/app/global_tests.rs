//! The vim profile's global keys at the app level (piece 4 spec §5.11;
//! plan M4b): `ZZ`/`ZQ`, the macro, mark and register notes, and the
//! recording flag; Task 3 adds the Manage screens' `q`.

use super::key_matrix_tests::{Fixture, build, draw, feed, snapshot};
use super::*;
use crate::config::KeyProfile;
use crate::keyroute::table::{NO_MACROS, NO_MARKS, NO_REGISTERS};

fn vim(f: Fixture) -> App {
    let mut app = build(f);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    app
}

fn toasts(app: &App) -> Vec<String> {
    app.toasts.messages().into_iter().map(str::to_string).collect()
}

#[test]
fn zz_saves_a_dirty_request_and_quits() {
    let mut app = vim(Fixture::TableThreeHeaders);
    assert!(app.editor.is_dirty(), "the fixture's headers are unsaved");
    feed(&mut app, "ZZ");
    assert!(app.should_quit);
    assert!(!app.editor.is_dirty(), "saved first");
}

#[test]
fn zq_quits_without_saving() {
    let mut app = vim(Fixture::TableThreeHeaders);
    let before = snapshot(&app);
    feed(&mut app, "ZQ");
    assert!(app.should_quit);
    assert!(app.editor.is_dirty());
    assert_eq!(snapshot(&app), before, "nothing written");
}

/// Review Focus 4.
#[test]
fn zz_on_a_scratch_asks_for_a_name_and_a_cancel_keeps_running() {
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, "ZZ");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "Save as");
    assert!(!app.should_quit);
    // Esc to Normal, Esc again cancels the prompt (spec §5.10).
    feed(&mut app, "<Esc><Esc>");
    assert!(app.modals.is_empty());
    assert!(!app.should_quit);
    assert_eq!(app.editor.url.text(), "https://x.test/scratch", "the edits stay");
}

#[test]
fn zz_in_an_open_url_line_saves_what_it_holds() {
    let mut app = vim(Fixture::UrlLine);
    feed(&mut app, "ix<Esc>ZZ");
    assert!(app.should_quit);
    assert!(!app.editor.is_dirty());
    assert!(app.editor.url.text().starts_with('x'), "{:?}", app.editor.url.text());
    assert!(!app.field_caret().is_live(), "closed before the save");
}

/// A dirty request behind the Settings tab, whose `osc52_limit` field is
/// open holding `abc`: a commit it refuses (the final review's I-2 trace).
pub(super) fn refused_settings_field() -> App {
    let mut app = vim(Fixture::TableThreeHeaders);
    app.update(Action::OpenManage { tab: Some(crate::components::manage::ManageTab::Settings) });
    app.settings.begin_edit(crate::components::settings::SettingsField::Osc52Limit, "abc");
    draw(&mut app);
    assert!(app.editor.is_dirty());
    assert!(app.field_caret().is_live());
    app
}

/// The final review's I-2 ruling: when the close `ZZ` starts with is
/// refused, the field stays open and nothing after the close runs.
#[test]
fn zz_on_a_field_that_refuses_its_commit_keeps_running() {
    let mut app = refused_settings_field();
    feed(&mut app, "ZZ");
    assert!(!app.should_quit, "the close was refused: no quit");
    assert_eq!(app.settings.editing, Some(crate::components::settings::SettingsField::Osc52Limit), "the field stays open");
    assert!(app.editor.is_dirty(), "nothing saved");
    assert!(app.modals.is_empty());
    // The leave rows halt the same way: `j` closes nothing and moves nothing.
    let row = app.settings.cursor;
    feed(&mut app, "j");
    assert_eq!(app.settings.editing, Some(crate::components::settings::SettingsField::Osc52Limit));
    assert_eq!(app.settings.cursor, row);
}

/// The code review of sweep backlog item 4: on Manage `ZQ` and `:q!` close
/// the screen abandoning what the field holds, as Vim's close a tab page
/// dropping a changed buffer's text (Vim 9.1 probe: both alike, nothing
/// written), so a field whose close refuses cannot keep the screen; the
/// request stays as it was. `:qa!` quits past it.
#[test]
fn zq_on_manage_abandons_a_field_that_refuses_its_close() {
    for q in ["ZQ", ":q!<CR>"] {
        let mut app = refused_settings_field();
        let limit = app.ui_settings.osc52_limit;
        feed(&mut app, q);
        assert_eq!(app.screen, Screen::Main, "{q}: Manage closed");
        assert!(!app.should_quit, "{q}");
        assert_eq!(app.settings.editing, None, "{q}: the field abandoned");
        assert_eq!(app.ui_settings.osc52_limit, limit, "{q}: nothing committed");
        assert!(app.editor.is_dirty(), "{q}: the request untouched");
        // A Variables edit is abandoned the same way, its text never
        // written, though it would commit.
        let mut app = vim(Fixture::VarForm);
        let before = snapshot(&app);
        feed(&mut app, "ixyz<Esc>");
        assert!(app.varmanager.form.editing.is_some());
        feed(&mut app, q);
        assert_eq!(app.screen, Screen::Main, "{q}");
        assert!(app.varmanager.form.editing.is_none(), "{q}: the edit abandoned");
        let diff = super::key_matrix_tests::snapshot_diff(&before, &snapshot(&app));
        assert!(!diff.contains("file "), "{q}: nothing written: {diff}");
    }
    let mut app = refused_settings_field();
    feed(&mut app, ":qa!<CR>");
    assert!(app.should_quit, ":qa! quits past a field that refuses its close");
}

/// The code review of sweep backlog item 4: a dialog over Manage takes the
/// window verbs as a dialog over the main screen does (they quit), as the
/// line's caption and the quit chip say; Manage is never closed under it.
#[test]
fn a_window_verb_in_a_dialog_over_manage_quits() {
    for q in [":q<CR>", ":q!<CR>"] {
        let mut app = vim(Fixture::VarList);
        feed(&mut app, "a");
        assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "{q}: the new-variable prompt");
        assert_eq!(app.surface(), crate::keyroute::Surface::Prompt);
        feed(&mut app, "<Esc>");
        feed(&mut app, q);
        assert!(app.should_quit, "{q}: quit, as over the main screen");
        assert_eq!(app.screen, Screen::Manage, "{q}: Manage not closed under the dialog");
    }
}

#[test]
fn zx_is_swallowed_on_a_list() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "Zxj");
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"));
    assert!(toasts(&app).is_empty());
    assert!(!app.should_quit);
}

/// Review Focus 3.
#[test]
fn the_recording_flag_swallows_the_closing_q() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "qa");
    assert!(toasts(&app).iter().any(|m| m.contains(NO_MACROS)));
    assert_eq!(app.key_echo(), "recording @a — not supported");
    assert_eq!(app.vim_status().as_deref(), Some("NORMAL  recording @a — not supported"));
    feed(&mut app, "x");
    feed(&mut app, "q");
    assert_eq!(app.key_echo(), "", "the closing q clears the flag");
    assert!(!app.should_quit);
    assert!(app.modals.is_empty());
    app.toasts = Default::default();
    feed(&mut app, "@a");
    assert!(toasts(&app).iter().any(|m| m.contains(NO_MACROS)));
    feed(&mut app, "j");
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"));
    feed(&mut app, "q");
    assert_eq!(app.key_echo(), "q", "a fresh prefix");
    feed(&mut app, "<Esc>");
    assert_eq!(app.key_echo(), "");
}

#[test]
fn the_recording_flag_in_an_open_field() {
    let mut app = vim(Fixture::UrlLine);
    feed(&mut app, "iab<Esc>qa");
    assert!(toasts(&app).iter().any(|m| m.contains(NO_MACROS)));
    assert!(app.field_caret().is_live(), "nothing closed");
    assert_eq!(app.key_echo(), "recording @a — not supported");
    feed(&mut app, "q");
    assert_eq!(app.key_echo(), "");
    feed(&mut app, "x");
    assert_eq!(app.editor.url.text(), "a", "x is the engine's again");
}

/// The final review's I-4 ruling: a field close drops the pending keys but
/// keeps the recording flag, so recording a cell edit and closing it with
/// `j` still ends at the next bare `q`, and `@a` only notes.
#[test]
fn the_recording_flag_survives_a_field_close() {
    let mut app = vim(Fixture::TableThreeHeaders);
    let rows = app.editor.headers.len();
    feed(&mut app, "qaix<Esc>");
    assert!(app.field_caret().is_live(), "the cell is open");
    feed(&mut app, "j");
    assert!(!app.field_caret().is_live(), "j closed the cell");
    assert_eq!(app.key_echo(), "recording @a — not supported", "the close kept the flag");
    feed(&mut app, "q");
    assert_eq!(app.key_echo(), "", "the closing q is swallowed and clears the flag");
    app.toasts = Default::default();
    feed(&mut app, "@a");
    assert!(toasts(&app).iter().any(|m| m.contains(NO_MACROS)), "{:?}", toasts(&app));
    assert_eq!(app.editor.headers.len(), rows, "nothing added");
    assert!(!app.field_caret().is_live(), "nothing opened");
    assert!(app.modals.is_empty());
}

/// Task 1's deferred case: the flag armed on one navigation surface holds
/// on the next, where the bare `q` closes it.
#[test]
fn the_recording_flag_survives_a_move_to_another_surface() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "qa");
    let from = app.surface();
    feed(&mut app, "<Tab>");
    assert_ne!(app.surface(), from, "Tab moved the keyboard");
    assert!(!app.field_caret().is_live());
    assert_eq!(app.key_echo(), "recording @a — not supported");
    feed(&mut app, "q");
    assert_eq!(app.key_echo(), "", "swallowed there, not a fresh prefix");
    assert!(!app.should_quit);
    assert!(app.modals.is_empty());
}

#[test]
fn marks_and_registers_note_on_a_list_and_the_next_key_is_fresh() {
    for (keys, note) in [("'a", NO_MARKS), ("`a", NO_MARKS), ("\"a", NO_REGISTERS)] {
        let mut app = vim(Fixture::SidebarThree);
        feed(&mut app, keys);
        assert!(toasts(&app).iter().any(|m| m.contains(note)), "{keys}: {:?}", toasts(&app));
        feed(&mut app, "j");
        assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"), "{keys}");
        assert!(app.modals.is_empty());
    }
    let mut app = vim(Fixture::UrlLine);
    feed(&mut app, "ima");
    feed(&mut app, "<Esc>ma");
    assert!(toasts(&app).iter().any(|m| m.contains(NO_MARKS)));
    assert_eq!(app.editor.url.text(), "ma", "nothing typed by the mark key");
}

/// Review Focus 5 (spec §4.5, plan M2 Deviation 2): the Manage and
/// Settings components no longer answer `q`; arrows reaches Quit through
/// the screen whitelist, and vim's `q` is the macro prefix there.
#[test]
fn q_quits_from_every_manage_surface_in_arrows_and_never_in_vim() {
    for f in [
        Fixture::ManageEnvs,
        Fixture::ManageEnvDetail,
        Fixture::ManageSpaces,
        Fixture::VarList,
        Fixture::VarGrid,
        Fixture::VarForm,
        Fixture::Settings,
        Fixture::SettingsFiles,
    ] {
        let mut app = build(f);
        draw(&mut app);
        feed(&mut app, "q");
        assert!(app.should_quit, "{f:?}: arrows q quits");
        let mut app = vim(f);
        feed(&mut app, "qa");
        assert!(!app.should_quit, "{f:?}: vim q never quits");
        assert!(toasts(&app).iter().any(|m| m.contains(NO_MACROS)), "{f:?}: {:?}", toasts(&app));
        assert_eq!(app.screen, Screen::Manage, "{f:?}");
    }
}
