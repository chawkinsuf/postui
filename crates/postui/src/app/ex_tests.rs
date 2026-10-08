//! The `:` command line at the app level (piece 4 spec §5.9; plan M4b).

use super::key_matrix_tests::{Fixture, build, draw, feed, snapshot};
use super::*;
use crate::components::palette::PaletteState;
use crate::config::KeyProfile;

fn vim(f: Fixture) -> App {
    let mut app = build(f);
    app.use_profile(KeyProfile::Vim);
    draw(&mut app);
    app
}

fn palette(app: &App) -> &PaletteState {
    match app.modals.top() {
        Some(Modal::Palette(p)) => p,
        _ => panic!("not the palette"),
    }
}

fn toasts(app: &App) -> Vec<String> {
    app.toasts.messages().into_iter().map(String::from).collect()
}

#[test]
fn colon_opens_the_line_in_insert_and_pins_the_verb() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":");
    assert!(palette(&app).is_ex());
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    feed(&mut app, "q");
    assert_eq!(palette(&app).pinned().map(|r| r.name), Some("quit"));
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty());
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/beta"), "nothing moved");
}

#[test]
fn enter_on_the_pinned_row_runs_it() {
    let mut app = vim(Fixture::TableThreeHeaders);
    assert!(app.editor.is_dirty());
    feed(&mut app, ":w<CR>");
    assert!(app.modals.is_empty());
    assert!(!app.editor.is_dirty());
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, ":q<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the dirty gate, not E37");
    assert!(!app.should_quit);
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, ":q!<CR>");
    assert!(app.should_quit);
    assert!(app.editor.is_dirty(), "unsaved");
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, ":wq<CR>");
    assert!(app.should_quit && !app.editor.is_dirty());
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, ":x<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "Save as");
    let mut app = vim(Fixture::SidebarThreeBetaOpen);
    feed(&mut app, ":q<CR>");
    assert!(app.should_quit, "a clean request quits at once");
}

/// The final review's I-1: `:q` meets the dirty gate from every state the
/// line opens from, a dialog under it included; ctrl+c keeps its modal
/// exemption, and `:q!` is the line's way out.
#[test]
fn colon_q_over_a_dialog_meets_the_dirty_gate() {
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, ":w<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "Save as");
    feed(&mut app, "<Esc>:q<CR>");
    assert!(!app.should_quit, "the gate, not a quit");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the dirty gate's confirm");
    feed(&mut app, "<Esc>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "cancelling the gate goes back to Save as");
    assert!(!app.should_quit);
    assert_eq!(app.mode_indicator(), Some("INSERT"), "its field back in Insert, as under any dialog that closes");
    feed(&mut app, "<Esc>:q!<CR>");
    assert!(app.should_quit, ":q! still leaves at once");
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, ":w<CR><C-c>");
    assert!(app.should_quit, "ctrl+c over a dialog stays an immediate exit");
}

/// Review Focus 1.
#[test]
fn a_parked_field_is_committed_before_the_verb() {
    let mut app = vim(Fixture::UrlLine);
    feed(&mut app, "ix<Esc>:w<CR>");
    assert!(app.modals.is_empty());
    assert!(!app.field_caret().is_live(), "closed");
    assert!(!app.editor.is_dirty(), "saved with the typed text");
    assert!(app.editor.url.text().starts_with('x'), "{:?}", app.editor.url.text());
}

/// The final review's I-2 ruling: a refused close halts the line's run,
/// so no verb acts on the stale text.
#[test]
fn a_refused_close_runs_no_verb() {
    for q in [":w<CR>", ":wq<CR>", ":x<CR>", ":q<CR>"] {
        let mut app = super::global_tests::refused_settings_field();
        feed(&mut app, q);
        assert!(app.modals.is_empty(), "{q}: {:?}", app.modals.top().map(std::mem::discriminant));
        assert_eq!(app.settings.editing, Some(crate::components::settings::SettingsField::Osc52Limit), "{q}: the field stays open");
        assert!(app.editor.is_dirty(), "{q}: nothing saved");
        assert!(!app.should_quit, "{q}");
    }
}

/// Review Focus 1.
#[test]
fn esc_on_the_line_returns_to_the_parked_field() {
    let mut app = vim(Fixture::UrlLine);
    feed(&mut app, "ix<Esc>");
    let text = app.editor.url.text().to_string();
    feed(&mut app, ":ab");
    // The line has the caret (`Live(Palette)`); the URL field waits under it.
    assert!(matches!(app.field_caret(), crate::keyroute::FieldCaret::Live(crate::keyroute::FieldId::Palette)));
    assert_eq!(app.screen_field(), Some(crate::keyroute::FieldId::Url));
    feed(&mut app, "<Esc>");
    assert!(app.modals.is_empty());
    assert!(app.field_caret().is_live(), "back in the field");
    assert_eq!(app.editor.url.text(), text);
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
}

#[test]
fn verbs_resolve_against_the_surface_under_the_line() {
    let mut app = vim(Fixture::TableThreeHeaders);
    let before = snapshot(&app);
    feed(&mut app, ":delete<CR>");
    assert_eq!(app.editor.headers.len(), 2);
    feed(&mut app, "u");
    assert_eq!(snapshot(&app), before, "undo restores exactly");
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":new<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
    let mut app = vim(Fixture::VarListGroup);
    feed(&mut app, ":fields<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::FieldsEditor(_))));
    let mut app = vim(Fixture::ManageEnvs);
    feed(&mut app, ":rename<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })));
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":fields");
    assert_eq!(palette(&app).refused(), Some("Nothing to :fields here"));
    feed(&mut app, "<CR>j");
    assert!(app.modals.is_empty());
    assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"));
}

/// Review Focus 2.
#[test]
fn unsupported_queries_run_nothing_and_never_pin_a_verb() {
    for q in [":s", ":se", ":set nu", ":%s/a/b/", ":5"] {
        let mut app = vim(Fixture::SidebarThree);
        feed(&mut app, q);
        assert!(palette(&app).pinned().is_none(), "{q}");
        assert!(palette(&app).refused().is_some(), "{q}");
        feed(&mut app, "<CR>j");
        assert!(app.modals.is_empty(), "{q}");
        assert!(toasts(&app).is_empty(), "{q}: {:?}", toasts(&app));
        assert_eq!(app.sidebar.selected_slug().as_deref(), Some("main/gamma"), "{q}");
        assert!(!app.should_quit, "{q}");
    }
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":sen");
    assert_eq!(palette(&app).pinned().map(|r| r.name), Some("send"));
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":zzzz<CR>");
    assert!(app.modals.is_empty());
    assert!(toasts(&app).iter().any(|m| m == "Not a command: :zzzz"), "{:?}", toasts(&app));
}

/// Key list X.edit: the line edits as any Insert field, and closes on
/// Backspace with nothing typed.
#[test]
fn the_line_edits_and_closes() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":ab cd<C-w>");
    assert_eq!(palette(&app).input(), "ab ");
    feed(&mut app, "<C-u>");
    assert_eq!(palette(&app).input(), "");
    feed(&mut app, "ab<BS>");
    assert_eq!(palette(&app).input(), "a");
    feed(&mut app, "<BS>");
    assert_eq!(palette(&app).input(), "");
    assert!(!app.modals.is_empty(), "the last char went; the line stays");
    feed(&mut app, "<BS>");
    assert!(app.modals.is_empty(), "Backspace on an empty line closes it");
}

#[test]
fn noh_clears_the_body_highlight() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "/a<CR>");
    assert!(app.glue.hlsearch());
    feed(&mut app, ":noh<CR>");
    assert!(app.modals.is_empty());
    assert!(!app.glue.hlsearch());
    assert!(app.field_caret().is_live(), "the body is not a field CloseField closes");
}

#[test]
fn ctrl_p_in_vim_opens_the_plain_palette() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, "<C-p>q");
    assert!(!palette(&app).is_ex());
    assert!(palette(&app).pinned().is_none());
}
