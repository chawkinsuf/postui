//! Dialogs, prompts, pickers and query boxes at the app level (piece 4
//! spec §5.10; plan M3b), in both profiles.

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

fn vim_app(f: Fixture) -> App {
    app_in(f, KeyProfile::Vim)
}

fn arrows_app(f: Fixture) -> App {
    app_in(f, KeyProfile::Arrows)
}

/// The top dialog's focused text, or "" when it has none.
fn prompt_text(app: &App) -> String {
    app.modals.focused_input().map(|i| i.text().to_string()).unwrap_or_default()
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

/// Key list D.prompt: a prompt opens in Insert, Esc goes to Normal, a
/// second Esc cancels.
#[test]
fn a_prompt_opens_in_insert_and_esc_twice_cancels() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r");
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    assert_eq!(app.modal_glue.session(), FieldSession::Live(FieldId::ModalInput(0)));
    feed(&mut app, "<Esc>");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert!(!app.modals.is_empty());
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty());
    assert_eq!(app.modal_glue.session(), FieldSession::None);
    assert!(has_request(&app, "main/beta"), "nothing renamed");
}

/// Key list D.prompt: Enter confirms from Insert and from Normal.
#[test]
fn enter_confirms_a_prompt_from_insert_and_from_normal() {
    for keys in ["r<Esc>ccdelta<CR>", "r<Esc>ccdelta<Esc><CR>"] {
        let mut app = vim_app(Fixture::SidebarThree);
        feed(&mut app, keys);
        assert!(app.modals.is_empty(), "{keys}");
        assert!(has_request(&app, "main/delta"), "{keys}: renamed");
    }
}

/// Spec §5.10: `j` in a prompt's idle Normal aims the button row.
#[test]
fn j_in_a_prompts_normal_aims_the_buttons() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Esc>j");
    assert_eq!(app.modals.button_focus(), Some(crate::components::modal::FormButton::Confirm));
    assert!(!app.field_caret().is_live());
}

/// Review Focus 3, plan M3b Deviation 11.
#[test]
fn an_engine_edit_closes_as_one_dialog_step() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Esc>x");
    assert_eq!(prompt_text(&app), "bet");
    feed(&mut app, "jk");
    assert!(!app.modals.field_open(), "back on the selected field");
    feed(&mut app, "u");
    assert_eq!(prompt_text(&app), "beta", "the dialog's own undo walks the field back");
}

/// Plan M3b Deviation 11: a cover over the field leaves its edit open, so
/// the close step still starts from the text the first open saw.
#[test]
fn a_cover_keeps_the_text_at_open() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Esc>x:<Esc><Esc>jk");
    feed(&mut app, "u");
    assert_eq!(prompt_text(&app), "beta");
}

/// Review Focus 1, plan M3b Deviation 1.
#[test]
fn a_yank_in_a_prompt_puts_in_the_url() {
    let mut app = vim_app(Fixture::UrlLine);
    app.editor.url = LineInput::new("ab");
    app.capture_undo();
    draw(&mut app);
    app.push_modal(Modal::Prompt {
        title: "Name".into(),
        input: LineInput::new("token"),
        kind: crate::components::modal::PromptKind::NewRequest,
        revealed: false,
    });
    draw(&mut app);
    feed(&mut app, "<Esc>0yiw<Esc>");
    assert!(app.modals.is_empty(), "Esc in idle Normal cancelled the prompt");
    feed(&mut app, "<CR>P");
    assert_eq!(app.editor.url.text(), "atokenb");
}

/// Review Focus 2: the palette over an open URL in Normal leaves its
/// engine state alone (spec §3.3 parking).
#[test]
fn the_palette_over_a_normal_url_leaves_it_as_it_was() {
    let mut app = vim_app(Fixture::UrlLine);
    app.editor.url = LineInput::new("ab");
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, "<CR>x");
    assert_eq!(app.editor.url.text(), "a");
    feed(&mut app, ":");
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    assert_eq!(app.glue.session(), FieldSession::Parked(FieldId::Url));
    feed(&mut app, "zz<Esc>");
    assert!(app.modals.is_empty());
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(app.editor.url.text(), "ab", "its engine history survived");
}

/// Plan M3b Deviation 1 (the user's decision): a dialog field covered by
/// another dialog comes back in Insert.
#[test]
fn a_prompt_under_the_palette_comes_back_in_insert() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r<Esc>:");
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    feed(&mut app, "<Esc>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    assert_eq!(prompt_text(&app), "beta");
}

/// Spec §3.3: Tab between a form's fields carries Insert.
#[test]
fn tab_carries_insert_between_a_forms_fields() {
    let mut app = vim_app(Fixture::SidebarThree);
    app.push_modal(Modal::NewProject {
        name: LineInput::new(""),
        path: LineInput::new("/tmp/"),
        on_path: false,
        prefilled: false,
    });
    draw(&mut app);
    feed(&mut app, "ab<Tab>cd");
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    assert_eq!(app.modal_glue.session(), FieldSession::Live(FieldId::ModalInput(1)));
    assert!(prompt_text(&app).ends_with("cd"), "{}", prompt_text(&app));
}

/// Plan M3b Deviation 6: a key the engine hands back reaches the dialog's
/// own arms and never its text.
#[test]
fn declined_keys_in_a_dialog_never_type() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, "r");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let input = app.modals.focused_input().unwrap();
    assert!(input.selection().is_none(), "ctrl+a is arrows' select-all");
    assert_eq!(input.cursor(), 4, "nor the caret moved");
    assert_eq!(prompt_text(&app), "beta");
    let mut form = vim_app(Fixture::SidebarThree);
    form.push_modal(Modal::NewProject {
        name: LineInput::new(""),
        path: LineInput::new("/tmp/"),
        on_path: false,
        prefilled: false,
    });
    draw(&mut form);
    form.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert!(matches!(form.modals.top(), Some(Modal::FilePicker(_))), "alt+b still browses");
}

/// The arrows profile has no dialog sessions: a prompt types as on main.
#[test]
fn arrows_dialogs_still_type_modelessly() {
    let mut app = arrows_app(Fixture::SidebarThree);
    feed(&mut app, "rx");
    assert_eq!(prompt_text(&app), "betax");
    assert_eq!(app.modal_glue.session(), FieldSession::None);
    assert_eq!(app.mode_indicator(), None);
}
