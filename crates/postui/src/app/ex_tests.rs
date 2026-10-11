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

/// Plan M5b Deviation 18: `!` writes over a file changed outside the app
/// with no dialog, as Vim's `:w!` and `:wq!` do; without it the drift
/// dialog asks, as before. The disk holds the app's content afterwards.
#[test]
fn bang_writes_over_a_file_changed_outside() {
    let outside = |app: &App| {
        let slug = app.editor.slug.clone().expect("the fixture's request is saved");
        let theirs = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/outside""#).unwrap();
        postui_core::fixtures::save_request(app.proj().root(), &slug, &theirs).unwrap();
    };
    let on_disk = |app: &App| {
        let slug = app.editor.slug.clone().unwrap();
        postui_core::fixtures::load_request(app.proj().root(), &slug).unwrap()
    };
    let mut app = vim(Fixture::TableThreeHeaders);
    outside(&app);
    feed(&mut app, ":wq<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the drift dialog");
    assert!(!app.should_quit);
    assert_eq!(on_disk(&app).url, "https://x/outside", "nothing written yet");
    for q in [":wq!<CR>", ":x!<CR>"] {
        let mut app = vim(Fixture::TableThreeHeaders);
        outside(&app);
        feed(&mut app, q);
        assert!(app.modals.is_empty(), "{q}: no dialog");
        assert!(!app.editor.is_dirty(), "{q}: written");
        assert!(app.should_quit, "{q}");
        assert_ne!(on_disk(&app).url, "https://x/outside", "{q}: the app's content is on disk");
    }
    let mut app = vim(Fixture::TableThreeHeaders);
    outside(&app);
    feed(&mut app, ":w!<CR>");
    assert!(app.modals.is_empty());
    assert!(!app.editor.is_dirty(), "written");
    assert!(!app.should_quit, ":w! does not quit");
    assert_ne!(on_disk(&app).url, "https://x/outside", "the app's content is on disk");
}

/// The M5b code review's second finding: `:w!` on a slug the project no
/// longer holds re-seeds it as `:w` does, so the drift check is live again
/// for the next save; a file that will not read is still written over.
#[test]
fn bang_holds_an_un_held_request_again() {
    let mut app = vim(Fixture::TableThreeHeaders);
    let slug = app.editor.slug.clone().expect("the fixture's request is saved");
    app.proj_mut().close_request(&slug);
    feed(&mut app, ":w!<CR>");
    assert!(!app.editor.is_dirty(), "written");
    assert!(app.proj().held_request(&slug).is_some(), "held again");
    let theirs = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/outside""#).unwrap();
    postui_core::fixtures::save_request(app.proj().root(), &slug, &theirs).unwrap();
    feed(&mut app, ":w<CR>");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the drift dialog");

    let mut app = vim(Fixture::TableThreeHeaders);
    let slug = app.editor.slug.clone().unwrap();
    app.proj_mut().close_request(&slug);
    std::fs::write(postui_core::storage::request_path(app.proj().root(), &slug), "url = \"unclosed\n").unwrap();
    feed(&mut app, ":w!<CR>");
    assert!(app.modals.is_empty(), "no dialog");
    assert!(!app.editor.is_dirty(), "written over");
    assert!(postui_core::fixtures::load_request(app.proj().root(), &slug).is_ok(), "the app's content is on disk");
}

/// Plan M5b Deviation 18: on a clean request `:x!` quits without writing
/// (a file changed outside stays as it is); a never-saved one asks for a
/// name, as without `!`.
#[test]
fn bang_on_a_clean_or_never_saved_request() {
    let mut app = vim(Fixture::SidebarThreeBetaOpen);
    let slug = app.editor.slug.clone().expect("the fixture's request is saved");
    let theirs = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/outside""#).unwrap();
    postui_core::fixtures::save_request(app.proj().root(), &slug, &theirs).unwrap();
    feed(&mut app, ":x!<CR>");
    assert!(app.should_quit, "a clean request quits at once");
    assert_eq!(
        postui_core::fixtures::load_request(app.proj().root(), &slug).unwrap().url,
        "https://x/outside",
        "nothing was written"
    );
    for q in [":wq!<CR>", ":w!<CR>"] {
        let mut app = vim(Fixture::DirtyScratch);
        feed(&mut app, q);
        assert!(matches!(app.modals.top(), Some(Modal::Prompt { .. })), "{q}: Save as");
        assert!(!app.should_quit, "{q}");
    }
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

/// Plan M5b Deviation 19: `:new` from an open cell opens the new row in
/// Insert, as from the closed table (spec §5.9, M4b's final review).
/// Before, the session carried the open cell's Normal to the new one.
#[test]
fn colon_new_from_an_open_cell_opens_in_insert() {
    let mut app = vim(Fixture::TableThreeHeaders);
    feed(&mut app, "i<Esc>");
    assert!(app.field_caret().is_live());
    assert_eq!(app.mode_indicator(), Some("NORMAL"), "the cell is open, in Normal");
    feed(&mut app, ":new<CR>");
    assert_eq!(app.editor.table.selected, Some(3), "the new row");
    assert!(app.field_caret().is_live());
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    let mut app = vim(Fixture::VarGrid);
    feed(&mut app, "i<Esc>:new<CR>");
    assert!(app.field_caret().is_live());
    assert_eq!(app.mode_indicator(), Some("INSERT"), "the grid's new option too");
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
    assert!(app.vim.search_memory.hlsearch());
    feed(&mut app, ":noh<CR>");
    assert!(app.modals.is_empty());
    assert!(!app.vim.search_memory.hlsearch());
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

/// The second code review's second finding: a typo no verb spells (`:W`)
/// is Vim's E492, a note, not whichever fuzzy row ranked first.
#[test]
fn a_typo_is_not_a_command() {
    for q in ["W", "Wq", "Qa", "QUIT"] {
        let mut app = vim(Fixture::TableThreeHeaders);
        let before = snapshot(&app);
        feed(&mut app, &format!(":{q}<CR>"));
        assert!(app.modals.is_empty(), "{q}: {:?}", app.modals.top().map(std::mem::discriminant));
        assert_eq!(snapshot(&app), before, "{q}");
        assert!(app.editor.is_dirty(), "{q}: nothing saved");
        assert!(!app.should_quit, "{q}");
        assert_eq!(toasts(&app), vec![format!("Not a command: :{q}")], "{q}");
    }
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
    click_chip(&mut app, Action::CloseWindow { write: false, force: false });
    assert!(!app.should_quit, "the gate, not a quit");
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the dirty gate's confirm");
    let mut app = build(Fixture::DirtyScratch);
    draw(&mut app);
    feed(&mut app, "<C-p>");
    click_chip(&mut app, Action::Quit);
    assert!(app.should_quit, "arrows' ^C chip stays the emergency exit over a dialog");
}

/// Sweep backlog item 4: on Manage, where `:q` closes the screen, the quit
/// chip reads `:qa` and quits as the typed `:qa` does.
#[test]
fn on_manage_the_quit_chip_is_qa() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = vim(Fixture::ManageEnvs);
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let r = app.hits.rect_of(&crate::hit::Hit::FooterChip(Action::GatedQuit)).expect("the quit chip runs :qa");
    let buf = terminal.backend().buffer();
    let y = r.y + r.height / 2;
    let text: String = (r.x..r.x + r.width).map(|x| buf.cell((x, y)).unwrap().symbol().to_string()).collect();
    assert!(text.contains(":qa"), "{text:?}");
    click_chip(&mut app, Action::GatedQuit);
    assert!(app.should_quit);
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

/// Plan M5b Deviations 13 and 17 (probes H1, H6, H7): the line keeps what
/// it closed with, Enter or Esc, an equal line moved to the newest place;
/// ↑ recalls them; Backspace on an empty line keeps nothing.
#[test]
fn the_line_keeps_its_history() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":abc<Esc>:noh<CR>:abc<Esc>");
    assert_eq!(app.vim.cmdline_history.lines(), ["noh", "abc"]);
    feed(&mut app, ":<Up>");
    assert_eq!(palette(&app).input(), "abc");
    assert_eq!(app.mode_indicator(), Some("INSERT"), "still typing");
    feed(&mut app, "<Up>");
    assert_eq!(palette(&app).input(), "noh");
    feed(&mut app, "<Esc>:<BS>");
    assert_eq!(app.vim.cmdline_history.lines(), ["abc", "noh"]);
}

/// The M5b code review's first finding: a click off the line abandons
/// it as Esc does, so the line is kept too (probe H6).
#[test]
fn a_click_off_the_line_keeps_it() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":send");
    click(&mut app, crate::hit::Hit::ModalOutside);
    assert!(app.modals.is_empty());
    assert_eq!(app.vim.cmdline_history.lines(), ["send"]);
}

/// Review Focus 3: a recalled line runs what it spells, as typed: a verb
/// pins, and text no verb spells is Vim's E492, never a fuzzy row.
#[test]
fn a_recalled_line_runs_what_it_spells() {
    let mut app = vim(Fixture::SidebarThree);
    feed(&mut app, ":noh<CR>:<Up>");
    assert_eq!(palette(&app).pinned().map(|r| r.name), Some("nohlsearch"));
    feed(&mut app, "<Esc>:W<CR>:<Up><CR>");
    assert!(app.modals.is_empty());
    let e492 = toasts(&app).iter().filter(|t| t.as_str() == "Not a command: :W").count();
    assert_eq!(e492, 2, "{:?}", toasts(&app));
}

/// Plan M5b Deviation 16: the history is written at quit, with the usage.
#[test]
fn quitting_writes_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = vim(Fixture::SidebarThree);
    app.config = crate::config::Config::at(dir.path().to_path_buf());
    feed(&mut app, ":noh<CR>:q<CR>");
    assert!(app.should_quit);
    let (_, loaded, _) = crate::config::Config::load_from(crate::config::Config::at(dir.path().to_path_buf()), false);
    assert_eq!(loaded.cmdline.lines(), ["noh", "q"]);
}
