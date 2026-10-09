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

/// The final review's I-3: an add verb opens what it makes in Insert from
/// the line as from its row (spec §5.6), on a table, the options grid and
/// the Variables list.
#[test]
fn the_add_verbs_open_in_insert_from_the_line_as_from_the_row() {
    for (f, row, line) in [
        (Fixture::TableThreeHeaders, "a", ":new<CR>"),
        (Fixture::VarGrid, "a", ":new<CR>"),
        (Fixture::VarListGroup, "A", ":group<CR>"),
    ] {
        let mut by_row = vim(f);
        feed(&mut by_row, row);
        let mut by_line = vim(f);
        feed(&mut by_line, line);
        assert!(by_line.field_caret().is_live(), "{f:?} {line}: {:?}", by_line.field_caret());
        assert_eq!(by_line.field_caret(), by_row.field_caret(), "{f:?}: {line} opens what {row} opens");
        assert_eq!(by_line.mode_indicator(), Some("INSERT"), "{f:?} {line}");
        assert_eq!(by_row.mode_indicator(), Some("INSERT"), "{f:?} {row}");
    }
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

/// The code review's third finding: `:` then Enter runs nothing, as in
/// Vim, though the line lists every command under it.
#[test]
fn colon_enter_runs_nothing() {
    let mut app = vim(Fixture::TableThreeHeaders);
    let before = snapshot(&app);
    feed(&mut app, ":<CR>");
    assert!(app.modals.is_empty());
    assert_eq!(snapshot(&app), before);
    assert!(toasts(&app).is_empty(), "{:?}", toasts(&app));
    assert!(!app.should_quit);
    assert!(app.editor.is_dirty(), "nothing saved or discarded");
}

/// The code review's first and second findings: `:u` is Vim's undo, not
/// a prefix of `:up`, and a verb given an argument or a trailing space is
/// never left to the fuzzy rows.
#[test]
fn undo_and_arguments_never_save_or_run_a_fuzzy_row() {
    for q in [":u<CR>", ":undo<CR>", ":w file<CR>", ":e foo.json<CR>"] {
        let mut app = vim(Fixture::TableThreeHeaders);
        let before = snapshot(&app);
        feed(&mut app, q);
        assert!(app.modals.is_empty(), "{q}");
        assert_eq!(snapshot(&app), before, "{q}");
        assert!(app.editor.is_dirty(), "{q}: nothing saved");
        assert!(toasts(&app).is_empty(), "{q}: {:?}", toasts(&app));
    }
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, ":w <CR>");
    assert!(!app.editor.is_dirty(), "a trailing space still saves");
}

fn click(app: &mut App, hit: crate::hit::Hit) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    draw(app);
    let r = app.hits.rect_of(&hit).unwrap_or_else(|| panic!("no {hit:?}"));
    app.handle_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: r.x + 1, row: r.y, modifiers: KeyModifiers::NONE });
}

fn click_chip(app: &mut App, action: Action) {
    click(app, crate::hit::Hit::FooterChip(action));
}

/// The second code review's first finding: the palette's own Quit row,
/// picked on the line over a dialog, meets the dirty gate as `:q` does,
/// rather than leaving at once with the edits lost.
#[test]
fn the_palette_quit_row_meets_the_dirty_gate_over_a_dialog() {
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, ":w<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "Save as");
    feed(&mut app, "<Esc>:quit<C-n>");
    assert_eq!(palette(&app).selected(), 1);
    assert_eq!(palette(&app).filtered()[0].id, "quit", "the fuzzy Quit row, under the pinned verb");
    feed(&mut app, "<CR>");
    assert!(!app.should_quit, "the gate, not a quit");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the dirty gate's confirm");
}

/// The second code review's fourth finding: a click on the pinned add
/// verb opens what it makes in Insert, as Enter on it does.
#[test]
fn a_clicked_add_verb_opens_in_insert_as_enter_does() {
    for (f, line) in [(Fixture::TableThreeHeaders, ":new"), (Fixture::VarGrid, ":new"), (Fixture::VarListGroup, ":group")] {
        let mut by_enter = vim(f);
        feed(&mut by_enter, &format!("{line}<CR>"));
        let mut by_click = vim(f);
        feed(&mut by_click, line);
        click(&mut by_click, crate::hit::Hit::PaletteRow(0));
        assert!(by_click.modals.top().is_none_or(|m| !matches!(m, Modal::Palette(_))), "{f:?}: the line closed");
        assert_eq!(by_click.field_caret(), by_enter.field_caret(), "{f:?}: {line} opens the same field clicked");
        assert_eq!(by_click.mode_indicator(), Some("INSERT"), "{f:?} {line}");
    }
}

/// The code review's fifth finding: where the quit chip reads `:q`, a
/// click runs `:q`'s gated quit, so over a dialog it meets the dirty
/// gate rather than leaving at once with the edits lost.
#[test]
fn the_quit_chip_runs_what_it_spells() {
    let mut app = vim(Fixture::DirtyScratch);
    feed(&mut app, ":w<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "Save as");
    draw(&mut app);
    assert!(app.hits.rect_of(&crate::hit::Hit::FooterChip(Action::Quit)).is_some(), "typing in the dialog: `^C`, the emergency exit");
    feed(&mut app, "<Esc>");
    click_chip(&mut app, Action::GatedQuit);
    assert!(!app.should_quit, "the gate, not a quit");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the dirty gate's confirm");
    let mut app = build(Fixture::DirtyScratch);
    draw(&mut app);
    feed(&mut app, "<C-p>");
    click_chip(&mut app, Action::Quit);
    assert!(app.should_quit, "arrows' ^C chip stays the emergency exit over a dialog");
}

/// The code review's sixth finding: the commands chip reads `:` in vim,
/// so a click opens the `:` line; in arrows it reads `^P` and opens the
/// plain palette.
#[test]
fn the_commands_chip_opens_the_palette_it_spells() {
    let mut app = vim(Fixture::SidebarThree);
    click_chip(&mut app, Action::OpenExPalette);
    assert!(palette(&app).is_ex());
    let mut app = build(Fixture::SidebarThree);
    click_chip(&mut app, Action::OpenPalette);
    assert!(!palette(&app).is_ex());
}
