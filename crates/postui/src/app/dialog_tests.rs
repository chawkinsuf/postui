//! Dialogs, prompts, pickers and query boxes at the app level (piece 4
//! spec §5.10; plan M3b), in both profiles.

use super::key_matrix_tests::{Fixture, build, draw, feed};
use super::*;
use crate::config::KeyProfile;

fn app_in(f: Fixture, profile: KeyProfile) -> App {
    let mut app = build(f);
    app.use_profile(profile);
    draw(&mut app);
    app
}

fn vim_app(f: Fixture) -> App {
    app_in(f, KeyProfile::Vim)
}

fn arrows_app(f: Fixture) -> App {
    app_in(f, KeyProfile::Arrows)
}

fn has_request(app: &App, slug: &str) -> bool {
    app.sidebar
        .rows
        .iter()
        .any(|r| matches!(r, crate::components::sidebar::Row::Request { slug: s, .. } if s == slug))
}

/// Spec §4.5: the dialogs' vim aliases leave arrows; vim's rows aim the
/// button row instead (plan M3b Deviation 19).
#[test]
fn h_and_l_aim_the_buttons_in_vim_only() {
    use crate::components::modal::FormButton;
    let mut vim = vim_app(Fixture::SidebarThree);
    feed(&mut vim, "r<Down>");
    assert_eq!(vim.modals.button_focus(), Some(FormButton::Confirm));
    feed(&mut vim, "l");
    assert_eq!(vim.modals.button_focus(), Some(FormButton::Cancel));
    let mut arrows = arrows_app(Fixture::SidebarThree);
    feed(&mut arrows, "r<Down>l");
    assert_eq!(arrows.modals.button_focus(), Some(FormButton::Confirm), "l is no longer an alias");
    feed(&mut arrows, "<Right>");
    assert_eq!(arrows.modals.button_focus(), Some(FormButton::Cancel));
}

/// Plan M3b Deviation 8: Enter presses the aimed button, so with Cancel
/// aimed nothing is renamed.
#[test]
fn enter_on_the_aimed_cancel_cancels() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Down>l<CR>");
    assert!(app.modals.is_empty());
    assert!(has_request(&app, "main/beta"));
}

/// Plan M3b Deviation 3: `i` reopens a selected prompt field; arrows has
/// no `i`.
#[test]
fn i_reopens_a_selected_prompt_field_in_vim() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Down>k");
    assert!(!app.modals.field_open(), "k from the buttons lands on the selected field");
    feed(&mut app, "i");
    assert!(app.modals.field_open());
    let mut arrows = arrows_app(Fixture::SidebarThree);
    feed(&mut arrows, "r<Down><Up>i");
    assert!(!arrows.modals.field_open(), "arrows' i is not an opener");
}

/// Plan M3b Deviation 7: `CloseField` closes a dialog's field to selected;
/// it never cancels the dialog.
#[test]
fn close_field_closes_a_dialog_field_and_keeps_the_dialog() {
    let mut app = arrows_app(Fixture::SidebarThree);
    feed(&mut app, "r");
    assert!(app.modals.field_open());
    app.update(Action::CloseField);
    assert!(!app.modals.is_empty() && !app.modals.field_open());
}

/// Spec §5.10: a confirm answers with its letters; vim's `j` is a move
/// there, and moving is all it does.
#[test]
fn a_confirm_answers_letters_and_ignores_moves() {
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let mut app = app_in(Fixture::SidebarThree, profile);
        app.push_modal(Modal::Confirm {
            title: "Sure?".into(),
            body: "b".into(),
            choices: vec![
                ('y', "Yes".into(), vec![Action::ShowToast("yes".into(), ToastKind::Info)]),
                ('n', "No".into(), vec![]),
            ],
        });
        feed(&mut app, "jx");
        assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "{profile:?}");
        feed(&mut app, "y");
        assert!(app.modals.is_empty(), "{profile:?}");
        assert!(app.toasts.messages().iter().any(|m| m.contains("yes")), "{profile:?}");
    }
}
