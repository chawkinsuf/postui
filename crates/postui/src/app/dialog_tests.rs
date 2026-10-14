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
    assert_eq!(app.vim.modal_glue.session(), FieldSession::Live(FieldId::ModalInput(0)));
    feed(&mut app, "<Esc>");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert!(!app.modals.is_empty());
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty());
    assert_eq!(app.vim.modal_glue.session(), FieldSession::None);
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

fn on_toggle(app: &App) -> bool {
    matches!(
        app.modals.top(),
        Some(Modal::Prompt {
            kind: crate::components::modal::PromptKind::NewSelector { on_toggle: true, .. },
            ..
        })
    )
}

fn new_selector_prompt(app: &mut App) {
    app.push_modal(Modal::Prompt {
        title: "New selector".into(),
        input: crate::components::line_input::LineInput::new(""),
        kind: crate::components::modal::PromptKind::NewSelector { shared: false, on_toggle: false },
        revealed: false,
    });
}

/// Final review M-1: Down from the open name field lands on the toggle in
/// both profiles; vim's `j` from Normal still does.
#[test]
fn down_from_the_new_selector_name_lands_on_the_toggle_in_both_profiles() {
    for keys in ["<Down>", "<Esc>j", "<Esc><Down>"] {
        let mut app = vim_app(Fixture::SidebarThree);
        new_selector_prompt(&mut app);
        feed(&mut app, keys);
        assert!(on_toggle(&app), "vim {keys}");
        assert!(!app.modals.field_open(), "vim {keys}: field closed");
        assert!(app.modals.button_focus().is_none(), "vim {keys}");
    }
    let mut app = arrows_app(Fixture::SidebarThree);
    new_selector_prompt(&mut app);
    feed(&mut app, "<Down>");
    assert!(on_toggle(&app), "arrows");
    assert!(!app.modals.field_open());
    assert!(app.modals.button_focus().is_none());
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
    assert_eq!(app.vim.glue.session(), FieldSession::Parked(FieldId::Url));
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
    assert_eq!(app.vim.modal_glue.session(), FieldSession::Live(FieldId::ModalInput(1)));
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

/// A dialog session whose dialog is gone is left through the ghost: its
/// index would name the field of the dialog now on top. Here the form
/// under the prompt has its focus on a choice field, so no field takes the
/// caret, and its text field 0 holds the prompt's text, so only the ghost
/// keeps the engine's Insert leave off it.
#[test]
fn a_popped_prompt_leaves_the_form_under_it_alone() {
    use crate::components::modal::{PromptField, PromptKind};
    let mut app = vim_app(Fixture::SidebarThree);
    app.push_modal(Modal::MultiPrompt {
        title: "Extract to variable".into(),
        fields: vec![
            PromptField::text("name", "Name", "seed"),
            PromptField::choice("destination", "Destination", &["Project default", "This request"]),
        ],
        focus: 0,
        kind: PromptKind::ExtractVariable,
    });
    draw(&mut app);
    feed(&mut app, "<Tab>");
    assert_eq!(app.modals.caret_field(), None, "the choice field is a stop");
    // Past the last char, where only a field the engine rests on is
    // clamped back (Normal's caret sits on a char).
    app.modals.text_input_mut(0).unwrap().set_cursor(4);
    let caret = app.modals.text_input_mut(0).map(|i| i.cursor());
    app.push_modal(Modal::Prompt {
        title: "Name".into(),
        input: LineInput::new("seed"),
        kind: PromptKind::NewRequest,
        revealed: false,
    });
    app.vim_catch_up();
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    app.update(Action::CancelTopModal);
    app.vim_catch_up();
    assert!(matches!(app.modals.top(), Some(Modal::MultiPrompt { .. })));
    assert_eq!(
        app.modals.text_input_mut(0).map(|i| i.cursor()),
        caret,
        "the form's field kept its caret"
    );
    assert_eq!(app.vim.modal_glue.session(), FieldSession::None);
    assert_eq!(app.vim.modal_session, None);
}

/// The arrows profile has no dialog sessions: a prompt types as on main.
#[test]
fn arrows_dialogs_still_type_modelessly() {
    let mut app = arrows_app(Fixture::SidebarThree);
    feed(&mut app, "rx");
    assert_eq!(prompt_text(&app), "betax");
    assert_eq!(app.vim.modal_glue.session(), FieldSession::None);
    assert_eq!(app.mode_indicator(), None);
}

/// One key as `feed` sends it, for a chord the notation cannot spell.
fn feed_key(app: &mut App, ev: KeyEvent) {
    app.handle_key(ev);
    app.capture_undo();
    draw(app);
}

/// A ready JSON response holding `body`, with the response pane focused.
fn json_response(app: &mut App, body: &str) {
    app.session.response.set_state(
        ResponseState::Ready(Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            sent_headers: Vec::new(),
            body: body.to_string(),
            ttfb: std::time::Duration::from_millis(1),
            elapsed: std::time::Duration::from_millis(1),
            size: body.len(),
            content_type: Some("application/json".into()),
        })),
        app.session.send_generation,
    );
    app.focus = PaneId::Response;
}

/// Key list D.picker: the palette query types through the engine.
#[test]
fn the_palette_query_types_through_the_engine() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, ":ab<BS>");
    let Some(Modal::Palette(p)) = app.modals.top() else { panic!("the palette") };
    assert_eq!(p.input(), "a");
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    assert_eq!(app.vim.modal_glue.session(), FieldSession::Live(FieldId::Palette));
    feed(&mut app, "<C-w>");
    let Some(Modal::Palette(p)) = app.modals.top() else { panic!("the palette") };
    assert_eq!(p.input(), "", "ctrl+w is the engine's Insert key");
}

/// Spec §3.7 item 2: no Normal layer, so `qa` types and Esc closes.
#[test]
fn a_query_box_types_letters_and_esc_closes_it() {
    let mut app = vim_app(Fixture::SidebarThree);
    feed(&mut app, ":qa");
    let Some(Modal::Palette(p)) = app.modals.top() else { panic!("the palette") };
    assert_eq!(p.input(), "qa");
    assert!(app.toasts.messages().is_empty(), "no macro note");
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty());
    assert_eq!(app.vim.modal_glue.session(), FieldSession::None);
}

/// Key list D.picker: ctrl+n and ctrl+p move the highlight.
#[test]
fn ctrl_n_and_ctrl_p_move_a_pickers_highlight() {
    let mut app = vim_app(Fixture::SidebarThree);
    // The first ctrl+n picks row 0 (Vim's wildmenu), then 1, 2, back to 1.
    feed(&mut app, ":<C-n><C-n><C-n><C-p>");
    let Some(Modal::Palette(p)) = app.modals.top() else { panic!("the palette") };
    assert_eq!(p.selected(), 1);
}

/// Task #6 (you, 2026-10-14): Left and Right flip the theme picker's
/// dark/light set in both profiles, typed filter or not, ahead of vim's
/// Insert caret; Tab still flips.
#[test]
fn left_and_right_flip_the_theme_picker_in_both_profiles() {
    for profile in [KeyProfile::Arrows, KeyProfile::Vim] {
        let mut app = app_in(Fixture::SidebarThree, profile);
        app.update(Action::ApplyTheme("gruvbox-dark".into()));
        app.update(Action::OpenThemeChooser);
        draw(&mut app);
        feed(&mut app, "<Right>");
        assert_eq!(app.theme_name, "gruvbox-light", "{profile:?}: Right flips");
        feed(&mut app, "<Left>");
        assert_eq!(app.theme_name, "gruvbox-dark", "{profile:?}: Left flips back");
        feed(&mut app, "gruv<Right>");
        assert_eq!(app.theme_name, "gruvbox-light", "{profile:?}: Right flips with a filter");
        let Some(Modal::Chooser(c)) = app.modals.top() else { panic!("the theme picker") };
        assert_eq!(c.input(), "gruv", "{profile:?}: the filter is untouched");
        feed(&mut app, "<Tab>");
        assert_eq!(app.theme_name, "gruvbox-dark", "{profile:?}: Tab still flips");
    }
}

/// Review Focus 5, plan M3b Deviation 5. Enter on a filtered folder
/// rebuilds the filter from "inn" to "": the session rebases on the new
/// text, and must stay Insert-only so Esc is still the picker's.
#[test]
fn the_file_picker_climbs_on_backspace_and_esc_still_closes() {
    let mut app = vim_app(Fixture::SidebarThree);
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    app.push_modal(Modal::FilePicker(crate::components::file_picker::FilePickerState::new(
        "Open",
        crate::components::file_picker::PickerTarget::OpenProject,
        dir.path(),
        "",
    )));
    draw(&mut app);
    feed(&mut app, "inn<CR>");
    let Some(Modal::FilePicker(p)) = app.modals.top() else { panic!("the file picker") };
    assert_eq!(p.dir(), inner.as_path(), "Enter went into the filtered folder");
    assert_eq!(p.input().text(), "", "and rebuilt the filter");
    feed(&mut app, "<BS>");
    let Some(Modal::FilePicker(p)) = app.modals.top() else { panic!("the file picker") };
    assert_eq!(p.dir(), dir.path(), "Backspace on an empty filter climbs");
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty(), "Esc is still the picker's after the rebuild");
}

/// Review Focus 4: the jq bar types through the engine, and its edit is
/// one app step, a segment delete (claimed by no one, declined by the
/// engine, taken by the bar) included (amendment R6).
#[test]
fn the_jq_bar_types_through_the_engine_as_one_app_step() {
    let mut app = vim_app(Fixture::ResponseLines);
    app.update(Action::OpenJqBar);
    // The main loop's capture after the event that opened the bar: the
    // app history's starting point.
    app.capture_undo();
    draw(&mut app);
    feed(&mut app, ".x");
    assert_eq!(app.session.response.jq_text(), ".x");
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Jq));
    assert_eq!(app.history.undo_len(), 0, "the engine's typing holds the gate");
    feed_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(app.session.response.jq_text(), ".", "the segment went");
    assert_eq!(app.history.undo_len(), 0, "the bar's own delete holds it too");
    feed(&mut app, "<Esc>");
    assert!(!app.session.response.jq_focused(), "Esc leaves the bar");
    assert_eq!(app.session.response.jq_text(), ".", "keeping its text");
    feed(&mut app, "u");
    assert_eq!(app.session.response.jq_text(), "", "one undo takes the whole edit back");
}

/// Reopening the bar over an applied filter puts the caret after its last
/// character, as the first open does: typed text appends, and `ctrl+u`
/// takes the typed text, then the line before it (Vim's Insert `ctrl+u`
/// with 'backspace' "start").
#[test]
fn reopening_the_jq_bar_seats_the_caret_after_the_filter() {
    let mut app = vim_app(Fixture::ResponseLines);
    app.update(Action::OpenJqBar);
    draw(&mut app);
    feed(&mut app, ".count<CR>");
    assert!(!app.session.response.jq_focused(), "Enter applies and leaves the bar");
    feed_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT));
    assert!(app.session.response.jq_focused(), "alt+q reopens the bar");
    feed(&mut app, "X");
    assert_eq!(app.session.response.jq_text(), ".countX");
    feed(&mut app, "<C-u>");
    assert_eq!(app.session.response.jq_text(), ".count", "the typed X");
    feed(&mut app, "<C-u>");
    assert_eq!(app.session.response.jq_text(), "", "then the line before it");
}

/// Plan M3b Deviation 5 (the user's decision): ctrl+backspace off a path
/// token falls back to the input's word delete in vim as in arrows.
#[test]
fn ctrl_backspace_off_a_path_deletes_a_word_in_both_profiles() {
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let mut app = app_in(Fixture::ResponseLines, profile);
        app.update(Action::OpenJqBar);
        app.capture_undo();
        draw(&mut app);
        feed(&mut app, ". | length");
        feed_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(app.session.response.jq_text(), ". | ", "{profile:?}");
    }
}

/// Plan M3b Deviation 13: a paste into the file picker keeps its jump.
#[test]
fn a_pasted_folder_jumps_the_file_picker() {
    let mut app = vim_app(Fixture::SidebarThree);
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    app.push_modal(Modal::FilePicker(crate::components::file_picker::FilePickerState::new(
        "Open",
        crate::components::file_picker::PickerTarget::OpenProject,
        dir.path(),
        "",
    )));
    draw(&mut app);
    app.paste_text(&inner.to_string_lossy());
    let Some(Modal::FilePicker(p)) = app.modals.top() else { panic!("the file picker") };
    assert_eq!(p.dir(), inner.as_path());
}

/// Amendment R4: the jq bar's Tab is the bar's before the engine, so menu
/// completion's rewrite of a key that needs quotes (it selects the token
/// through the input's own keys) still lands.
#[test]
fn the_jq_bars_tab_completes_a_quoted_key_in_vim() {
    let mut app = vim_app(Fixture::SidebarThree);
    json_response(&mut app, r#"{"my key": 1}"#);
    app.session.response.set_jq_tab(crate::config::JqTab::Menu);
    app.update(Action::OpenJqBar);
    draw(&mut app);
    feed(&mut app, ".my<Tab>");
    assert_eq!(app.session.response.jq_text(), ".\"my key\"");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Jq));
    feed(&mut app, "z");
    assert_eq!(
        app.session.response.jq_text(),
        ".\"my key\"z",
        "the engine types on after the rewrite"
    );
}

/// Arrows parity (Task 4 review): any key the engine takes leaves the jq
/// bar's entered menu row keeping the candidate, a caret move included, so
/// the Esc after it leaves the bar instead of un-picking.
#[test]
fn a_move_leaves_the_jq_menu_row_keeping_the_candidate() {
    for profile in [KeyProfile::Vim, KeyProfile::Arrows] {
        let mut app = app_in(Fixture::SidebarThree, profile);
        json_response(&mut app, r#"{"my key": 1, "myth": 2}"#);
        app.session.response.set_jq_tab(crate::config::JqTab::Menu);
        app.update(Action::OpenJqBar);
        draw(&mut app);
        feed(&mut app, ".my<Tab>");
        assert!(app.session.response.jq_menu_open(), "{profile:?}: Tab entered the row");
        assert_eq!(app.session.response.jq_text(), ".\"my key\"", "{profile:?}");
        feed(&mut app, "<Left>");
        assert!(!app.session.response.jq_menu_open(), "{profile:?}: the move left the row");
        feed(&mut app, "<Esc>");
        assert!(!app.session.response.jq_focused(), "{profile:?}: Esc left the bar");
        assert_eq!(
            app.session.response.jq_text(),
            ".\"my key\"",
            "{profile:?}: keeping the candidate"
        );
    }
}

/// Plan M3b Deviation 12: ctrl+z in a vim query box does nothing: not the
/// box's text, not the app history (a table row delete waits there).
#[test]
fn ctrl_z_in_a_vim_query_box_does_nothing() {
    let mut app = vim_app(Fixture::TableAfterDelete);
    let (rows, steps) = (app.editor.headers.len(), app.history.undo_len());
    feed(&mut app, ":ab<C-z>");
    let Some(Modal::Palette(p)) = app.modals.top() else { panic!("the palette") };
    assert_eq!(p.input(), "ab");
    assert_eq!((app.editor.headers.len(), app.history.undo_len()), (rows, steps));
}

/// Plan M3b Deviation 16: a click from the jq bar into the URL line is a
/// close and an open, so the URL line is not Insert-only: it opens in
/// Normal, where `x` deletes.
#[test]
fn a_click_from_the_jq_bar_into_the_url_does_not_carry_insert_only() {
    let mut app = vim_app(Fixture::UrlLine);
    app.editor.url = LineInput::new("ab");
    app.capture_undo();
    json_response(&mut app, "[1]");
    app.update(Action::OpenJqBar);
    draw(&mut app);
    feed(&mut app, ".");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Jq));
    let area = app.editor.last_url_text_area.expect("the URL line was drawn");
    app.handle_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Left,
        ),
        column: area.x,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "x");
    assert_eq!(app.editor.url.text(), "b", "Normal's x deletes");
}

/// Amendment R8 (the user's decision): `G` and `gg` move the variable
/// picker's option cursor, through the End and Home it gained.
#[test]
fn g_and_gg_move_the_variable_pickers_option_cursor() {
    use crate::components::var_picker::{SelectOption, VarPickerState};
    let mut app = vim_app(Fixture::SidebarThree);
    let option = |key: &str, selected| SelectOption {
        key: key.into(),
        description: None,
        value: None,
        selected,
        values: None,
    };
    app.push_modal(Modal::VarPicker(VarPickerState::new_select(
        vec![option("alice", false), option("bob", true), option("carol", false)],
        "user".into(),
        "identity".into(),
        "qa".into(),
    )));
    draw(&mut app);
    let selected = |app: &App| match app.modals.top() {
        Some(Modal::VarPicker(p)) => p.selected(),
        _ => panic!("the variable picker"),
    };
    assert_eq!(selected(&app), 1);
    feed(&mut app, "G");
    assert_eq!(selected(&app), 3, "the last row");
    feed(&mut app, "gg");
    assert_eq!(selected(&app), 0, "the first option");
}

fn secret_prompt(app: &mut App) {
    app.push_modal(Modal::Prompt {
        title: "Secret".into(),
        input: crate::components::line_input::LineInput::new("s3cret"),
        kind: crate::components::modal::PromptKind::SecretValue {
            name: "t".into(),
            env: "dev".into(),
            then_send: true,
        },
        revealed: false,
    });
    draw(app);
}

fn revealed(app: &App) -> bool {
    matches!(app.modals.top(), Some(Modal::Prompt { revealed: true, .. }))
}

/// Plan M3b Deviation 4 (the user's decision): ctrl+r reveals in both
/// profiles, never sends, and in vim beats the engine in either mode.
#[test]
fn ctrl_r_reveals_the_secret_prompt_in_both_profiles() {
    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    let mut arrows = arrows_app(Fixture::SidebarThree);
    secret_prompt(&mut arrows);
    arrows.handle_key(ctrl_r);
    assert!(revealed(&arrows), "arrows: revealed, not sent");
    let mut vim = vim_app(Fixture::SidebarThree);
    secret_prompt(&mut vim);
    vim.handle_key(ctrl_r);
    assert!(revealed(&vim), "vim Insert");
    feed(&mut vim, "<Esc>");
    vim.handle_key(ctrl_r);
    assert!(
        matches!(vim.modals.top(), Some(Modal::Prompt { revealed: false, .. })),
        "vim Normal: hidden again, the prompt still up, no redo"
    );
}

/// Main's reveal arm matched `Char('r' | 'R')` with CONTROL contained, so
/// ctrl+shift+r revealed in arrows; the claim keeps that.
#[test]
fn ctrl_shift_r_reveals_the_secret_prompt_in_arrows() {
    let mut arrows = arrows_app(Fixture::SidebarThree);
    secret_prompt(&mut arrows);
    arrows
        .handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
    assert!(revealed(&arrows));
    arrows
        .handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
    assert!(matches!(arrows.modals.top(), Some(Modal::Prompt { revealed: false, .. })));
}

/// Spec §10 open question 2 in the app: arrows' Enter renames at once.
#[test]
fn arrows_enter_confirms_a_rename() {
    let mut app = arrows_app(Fixture::SidebarThree);
    feed(&mut app, "r<C-u>delta<CR>");
    assert!(app.modals.is_empty());
    assert!(has_request(&app, "main/delta"));
}
