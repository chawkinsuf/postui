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
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert_eq!(url(&app), ("https://x.test/ab", 16), "a Normal caret rests on the last char");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "https://x.test/a");
    feed(&mut app, "<Esc>");
    assert!(!app.field_caret().is_live(), "Esc in idle Normal closes the field");
    assert_eq!(app.editor.sub_focus, SubFocus::Url, "the line stays selected");
    assert_eq!(url(&app).0, "https://x.test/a", "the text is kept");
    assert_eq!(app.vim.glue.session(), FieldSession::None);
}

/// Vim's Esc in Normal moves nothing: on the closed URL line, the method
/// badge and a closed table it leaves the selection where it is.
#[test]
fn esc_on_a_closed_url_line_method_or_table_stays_put() {
    let mut app = url_line("https://x.test/ab");
    feed(&mut app, "i<Esc><Esc>");
    assert!(!app.field_caret().is_live(), "the second Esc closed the line");
    feed(&mut app, "<Esc>");
    assert_eq!(app.editor.sub_focus, SubFocus::Url, "the URL line stays selected");
    feed(&mut app, "h<Esc>");
    assert_eq!(app.editor.sub_focus, SubFocus::Method, "the method badge stays selected");
    feed(&mut app, "j");
    assert_eq!(app.editor.sub_focus, SubFocus::Content);
    let Some(Stop::Row(row)) = app.editor.table.selected() else {
        panic!("j lands on a row: {:?}", app.editor.table.selected())
    };
    feed(&mut app, "<Esc>");
    assert_eq!(app.editor.table.selected(), Some(Stop::Row(row)), "the table row stays selected");
    feed(&mut app, "k");
    assert_eq!(app.editor.table.selected(), Some(Stop::Row(row - 1)), "and k goes on from it");
}

/// Vim's `cc`, `S` and `C` on the closed URL line change it as on an open
/// one: the text goes (from the caret for `C`) into the unnamed register,
/// and the line opens in Insert.
#[test]
fn cc_s_and_c_change_the_closed_url_line() {
    for (keys, want, register) in
        [("cc", "Z", "ab cd\n"), ("S", "Z", "ab cd\n"), ("C", "aZ", "b cd")]
    {
        let mut app = url_line("ab cd");
        app.editor.url.set_cursor(1);
        feed(&mut app, keys);
        assert!(app.field_caret().is_live(), "{keys}: the line opened");
        assert_eq!(app.mode_indicator(), Some("INSERT"), "{keys}");
        feed(&mut app, "Z");
        assert_eq!(url(&app).0, want, "{keys}");
        assert_eq!(app.vim.glue.unnamed_text(), register, "{keys}: the register");
        feed(&mut app, "<Esc>u");
        assert_eq!(url(&app).0, "ab cd", "{keys}: one undo in the field takes it back");
    }
    // The engine reads the count: `2cc` on the one line fails, as Vim's
    // does (probed: the line and the register unchanged).
    let mut app = url_line("ab cd");
    feed(&mut app, "2cc");
    assert_eq!((url(&app).0, app.mode_indicator()), ("ab cd", Some("NORMAL")));
}

/// The sidebar's dirty dot follows the field's text key by key: on with
/// the first typed char, off again once `u` brings back the saved text,
/// while the field is still open (Vim clears 'modified' at once).
#[test]
fn the_dirty_dot_follows_an_undo_back_to_the_saved_text() {
    // The main loop's tick refreshes the dot between keys.
    for f in [Fixture::UrlLine, Fixture::Body] {
        let mut app = vim(f);
        app.editor.mark_saved();
        feed(&mut app, "Ax");
        app.update(Action::Tick);
        assert!(app.sidebar.open_dirty, "{f:?}: typing dirties");
        feed(&mut app, "<Esc>u");
        app.update(Action::Tick);
        assert!(app.field_caret().is_live(), "{f:?}: the field is still open");
        assert!(!app.sidebar.open_dirty, "{f:?}: the dot goes with the undo");
    }
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
    assert_eq!(
        (app.mode_indicator(), url(&app).1),
        (Some("NORMAL"), 3),
        "Esc steps the caret back"
    );
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
    assert_eq!(app.editor.table.selected(), Some(Stop::Row(2)));
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
    assert!(matches!(
        app.vim.glue.session(),
        FieldSession::Live(FieldId::TableCell { col: 1, .. })
    ));
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
    assert_eq!(app.vim.glue.session(), FieldSession::None);
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
    assert_eq!(app.vim.glue.session(), FieldSession::Parked(FieldId::Url));
    feed(&mut app, "<Esc>");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
}

/// Plan M3a Deviations 12 and 17.
#[test]
fn a_settings_field_opens_in_insert_with_i() {
    use crate::components::settings::{SettingsField, SettingsRow, SettingsTab};
    let mut app = vim(Fixture::Settings);
    let row = SettingsTab::rows()
        .iter()
        .position(|r| *r == SettingsRow::Setting(SettingsField::AiCmd))
        .unwrap();
    app.settings.set_cursor(row);
    feed(&mut app, "i");
    assert!(matches!(app.vim.glue.session(), FieldSession::Live(FieldId::Settings(_))));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
    let seed = app.settings.field_text().to_string();
    feed(&mut app, "zz");
    assert_eq!(
        app.settings.field_text(),
        format!("{seed}zz"),
        "typing appends: arrows' select-all is gone"
    );
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
    let at =
        |kind| MouseEvent { kind, column: area.x + 3, row: area.y, modifiers: KeyModifiers::NONE };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    assert_eq!(url(&app).1, 3);
}

/// Review Focus 5 and spec §4.3.
#[test]
fn switching_profiles_with_a_field_open_keeps_its_text() {
    use super::vim_seam::Cause;
    let mut app = url_line("abc");
    feed(&mut app, "<CR>");
    app.use_profile(KeyProfile::Arrows);
    app.sync_session(None, Cause::Before);
    assert_eq!(app.vim.glue.session(), FieldSession::None);
    assert!(app.field_caret().is_live(), "the field stays open");
    assert_eq!(url(&app), ("abc", 2), "the text and caret stay");
    app.use_profile(KeyProfile::Vim);
    app.sync_session(None, Cause::Before);
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Url));
    assert_eq!(app.mode_indicator(), Some("INSERT"), "arrows to vim enters Insert");
    assert_eq!(url(&app), ("abc", 2));
}

/// Spec §4.3 ("the caret untouched"): leaving Insert for the switch must
/// not step the caret back off the end of the line.
#[test]
fn switching_to_arrows_in_insert_keeps_the_caret_at_the_end() {
    use super::vim_seam::Cause;
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
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    app.focus = PaneId::Sidebar;
    feed(&mut app, "j");
    assert_eq!(app.vim.glue.session(), FieldSession::None, "the body's session closed");
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
    assert!(app.vim.glue.can_undo(EngineKind::Line));
    app.editor.url = LineInput::new("wxyz");
    feed(&mut app, "0");
    assert!(!app.vim.glue.can_undo(EngineKind::Line), "the old history is gone");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "xyz", "the next key edits the new text");
}

/// Final review, the user's ruling on plan M3a Deviation 5: text replaced
/// under Insert ctrl+o re-enters as Insert, as Insert itself does.
#[test]
fn a_rebase_under_insert_ctrl_o_resumes_insert() {
    use super::vim_seam::Cause;
    let mut app = url_line("abc");
    feed(&mut app, "<CR>A<C-o>");
    assert_eq!(app.mode_indicator(), Some("(insert)"));
    app.editor.url = LineInput::new("wxyz");
    app.sync_session(None, Cause::Before);
    assert!(!app.vim.glue.can_undo(EngineKind::Line), "the session started over");
    assert_eq!(app.mode_indicator(), Some("INSERT"));
}

/// Final review T4: the user's terminal sends hover as `Drag(Right)`, so
/// only a left-button drag (and any press or release) syncs the session.
#[test]
fn a_right_button_drag_does_not_sync_the_session() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = url_line("abc");
    feed(&mut app, "<CR>x");
    assert!(app.vim.glue.can_undo(EngineKind::Line));
    app.editor.url = LineInput::new("wxyz");
    let hover = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Right),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(hover);
    assert!(app.vim.glue.can_undo(EngineKind::Line), "no sync ran, so no rebase");
    feed(&mut app, "0");
    assert!(!app.vim.glue.can_undo(EngineKind::Line), "a key's sync still rebases");
}

/// Final review I-3: a press-drag that opens a closed field selects, as it
/// does on an open one (and as arrows does). Task #15: from the pressed
/// char to the one under the pointer, both taken, as Vim's Visual is.
#[test]
fn a_press_drag_over_the_closed_url_selects() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = url_line("https://x.test/ab");
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    let at = |kind, col| MouseEvent {
        kind,
        column: area.x + col,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 2));
    draw(&mut app);
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 8));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 8));
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.editor.url.paint_span(), Some((2, 9)), "`tps://x`");
}

/// I-3's body twin: the body opened by a press from another pane.
#[test]
fn a_press_drag_into_the_body_from_another_pane_selects() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = vim(Fixture::Body);
    app.focus = PaneId::Sidebar;
    let buf = draw_buffer(&mut app);
    assert_eq!(app.vim.glue.session(), FieldSession::None);
    let area = app.editor.last_body_area.expect("the body is drawn");
    let y = area.y + 1;
    let x = (area.x..area.right())
        .find(|&x| buf[(x, y)].symbol() == "\"")
        .expect("row 1 shows `\"a\"`");
    let at = |kind, x| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), x));
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    draw(&mut app);
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), x + 2));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), x + 2));
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    let span = app.editor.body_visual.map(|(a, b)| ((a.row, a.col), (b.row, b.col)));
    assert_eq!(span, Some(((1, 2), (1, 4))), "`\"a\"`");
}

/// The URL line's mouse at char `col` (it draws from char 0 while short).
/// No draw between events, so a multi-click stays inside the click window.
fn url_mouse(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, col: u16) {
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    app.handle_mouse(ratatui::crossterm::event::MouseEvent {
        kind,
        column: area.x + col,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    });
}

/// A left press and release at a screen cell.
fn click_at(app: &mut App, column: u16, row: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        app.handle_mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
    }
}

/// Task #15 (spec R12, Q1 (b)) and task #23 (A7): one record per press
/// gives the list count (same hit, 400 ms, a pair), which rows and buttons
/// read in both profiles, and the text count (same hit and cell, 500 ms,
/// up to four), which every field's text reads in both profiles.
#[test]
fn a_press_counts_both_ways() {
    use super::mouse::{Clicks, count_clicks};
    let t0 = std::time::Instant::now();
    let mut last = None;
    let mut press_on = |hit: Hit, cell: (u16, u16), ms: u64| {
        let rec =
            count_clicks(last.as_ref(), &hit, cell, t0 + std::time::Duration::from_millis(ms));
        let clicks = rec.clicks;
        last = Some(rec);
        clicks
    };
    assert_eq!(press_on(Hit::SendButton, (5, 1), 0), Clicks { list: 1, text: 1 });
    assert_eq!(
        press_on(Hit::UrlBar, (5, 1), 50),
        Clicks { list: 1, text: 1 },
        "the same cell under another control: a first click"
    );
    let mut press = |cell: (u16, u16), ms: u64| press_on(Hit::UrlBar, cell, ms + 1000);
    assert_eq!(press((5, 1), 0), Clicks { list: 1, text: 1 });
    assert_eq!(press((5, 1), 100), Clicks { list: 2, text: 2 });
    assert_eq!(press((5, 1), 200), Clicks { list: 1, text: 3 }, "lists pair, text counts on");
    assert_eq!(press((5, 1), 300), Clicks { list: 2, text: 4 });
    assert_eq!(press((5, 1), 400), Clicks { list: 1, text: 1 }, "after four text starts over");
    assert_eq!(press((6, 1), 450), Clicks { list: 2, text: 1 }, "another cell, same hit");
    assert_eq!(press((6, 1), 900), Clicks { list: 1, text: 2 }, "450 ms: past 400, inside 500");
    assert_eq!(press((6, 1), 1500), Clicks { list: 1, text: 1 });
}

/// Task #15: a table cell's double click is Vim's word in vim (arrows
/// takes the punctuation run `.:`, Vim each punctuation char alone).
#[test]
fn a_double_click_in_a_cell_selects_vims_word() {
    let mut app = vim(Fixture::TableThreeHeaders);
    app.editor
        .headers
        .insert("a".into(), postui_core::model::Entry { value: "x.:y".into(), enabled: true });
    draw(&mut app);
    let cell = app.hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).expect("the value cell");
    click_at(&mut app, cell.x + 1, cell.y);
    draw(&mut app);
    let cell = app.hits.rect_of(&Hit::TableCell { row: 0, col: 1 }).expect("the value cell");
    click_at(&mut app, cell.x + 1, cell.y);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.active_selection_text().as_deref(), Some("."));
}

/// Task #15: a dialog's text box takes Vim's double click too.
#[test]
fn a_double_click_in_a_dialog_box_selects_vims_word() {
    let mut app = vim(Fixture::UrlLine);
    app.anims.enabled = false;
    app.update(Action::PromptNewRequest);
    app.paste_text("ab.cd");
    draw(&mut app);
    let r = app.hits.rect_of(&Hit::ModalInput(0)).expect("the prompt's box");
    click_at(&mut app, r.x + 2 + 3, r.y + 1);
    click_at(&mut app, r.x + 2 + 3, r.y + 1);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.active_selection_text().as_deref(), Some("cd"));
}

/// Task #15: the body's double click is Vim's word: `:` alone.
#[test]
fn a_double_click_in_the_body_selects_vims_word() {
    let mut app = vim(Fixture::Body);
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.expect("the body is drawn");
    let y = area.y + 1;
    let x = (area.x..area.right())
        .find(|&x| buf[(x, y)].symbol() == ":")
        .expect("row 1 shows `\"a\": 1`");
    click_at(&mut app, x, y);
    click_at(&mut app, x, y);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    let span = app.editor.body_visual.map(|(a, b)| ((a.row, a.col), (b.row, b.col)));
    assert_eq!(span, Some(((1, 5), (1, 5))));
}

/// Review round 1, finding 1: a right press decided once, after the menu
/// over the field closed, extends the field it lands on.
#[test]
fn a_right_press_through_a_dropdown_extends_the_field_under_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 2);
    app.update(Action::OpenMethodDropdown);
    draw(&mut app);
    assert!(!app.modals.is_empty(), "the dropdown is up");
    url_mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 9);
    url_mouse(&mut app, MouseEventKind::Up(MouseButton::Right), 9);
    assert!(app.modals.is_empty(), "the dropdown closed and no menu opened");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.editor.url.paint_span(), Some((2, 10)));
}

/// Review round 1, finding 2: a right press the field cannot map to a char
/// extends nothing; the menu opens as elsewhere.
#[test]
fn a_right_press_with_no_char_under_it_opens_the_menu() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 2);
    let bar = app.hits.rect_of(&Hit::UrlBar).expect("the URL bar");
    app.editor.last_url_text_area = None;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: bar.x + 1,
        row: bar.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(!app.modals.is_empty(), "the Copy/Paste menu");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
}

/// A left click (press and release) on the URL line's char `col`.
fn url_click(app: &mut App, col: u16) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    url_mouse(app, MouseEventKind::Down(MouseButton::Left), col);
    url_mouse(app, MouseEventKind::Up(MouseButton::Left), col);
}

/// Task #15: Vim's double click selects the word, and the release keeps it
/// (it used to settle as a click, which ended the Visual).
#[test]
fn a_double_click_selects_vims_word_and_the_release_keeps_it() {
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 5);
    url_click(&mut app, 5);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.editor.url.paint_span(), Some((4, 7)), "`bar`: the `.` is a word of its own");
    assert_eq!(app.active_selection_text().as_deref(), Some("bar"), "what ctrl+c copies");
}

/// Task #15: Vim counts a double click on one cell only.
#[test]
fn presses_on_two_cells_are_two_clicks() {
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 5);
    url_click(&mut app, 9);
    assert_eq!((app.mode_indicator(), url(&app).1), (Some("NORMAL"), 9));
}

/// Task #15: a drag after a double click goes by Vim's words, in a
/// one-line field too (its word anchor used to go with the press).
#[test]
fn a_drag_after_a_double_click_goes_by_words() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 5);
    url_mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 5);
    url_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 9);
    url_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 13);
    url_mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 13);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.editor.url.paint_span(), Some((4, 15)), "`bar.baz qux`");
}

/// Task #15: Vim's triple click selects the line.
#[test]
fn a_triple_click_selects_the_line() {
    let mut app = url_line("foo bar.baz qux");
    for _ in 0..3 {
        url_click(&mut app, 5);
    }
    assert_eq!(app.mode_indicator(), Some("V-LINE"));
    assert_eq!(app.editor.url.paint_span(), Some((0, 15)));
}

/// Task #15 (the user's ruling): in vim a right press on the field being
/// edited extends its selection, as Vim's `'mousemodel'` `extend` does,
/// rather than opening the Copy/Paste menu.
#[test]
fn a_right_press_on_the_open_field_extends() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 2);
    url_mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 9);
    url_mouse(&mut app, MouseEventKind::Up(MouseButton::Right), 9);
    assert!(app.modals.is_empty(), "no menu");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    assert_eq!(app.editor.url.paint_span(), Some((2, 10)));
    url_mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 12);
    assert_eq!(app.editor.url.paint_span(), Some((2, 10)), "the right press armed no sweep");
}

/// Task #15: a mouse selection made in Insert runs inside `ctrl+o`, so Esc
/// gives Insert back, as in Vim.
#[test]
fn esc_after_a_mouse_selection_in_insert_goes_back_to_insert() {
    let mut app = url_line("foo bar.baz qux");
    url_click(&mut app, 2);
    feed(&mut app, "i");
    url_click(&mut app, 5);
    url_click(&mut app, 5);
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    feed(&mut app, "<Esc>");
    assert_eq!((app.mode_indicator(), url(&app).1), (Some("INSERT"), 6));
}

/// Final review I-4: a one-line field has no prompt row, so the footer
/// echo shows the pattern as it is typed.
#[test]
fn a_search_in_a_one_line_field_echoes_its_pattern() {
    let mut app = url_line("foo bar");
    feed(&mut app, "<CR>0/fo");
    assert_eq!(app.key_echo(), "/fo");
    assert_eq!(app.vim_status().as_deref(), Some("SEARCH  /fo"));
    feed(&mut app, "<Esc>");
    assert_eq!(app.key_echo(), "");
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
    assert!(!app.vim.glue.can_undo(EngineKind::Line), "the session started over on the new text");
    feed(&mut app, "x");
    assert_eq!(url(&app).0, "ab", "the next x deletes under the caret on the new text");
}

/// Confirms the palette with `action`, as its Enter would (`:` parks the
/// open field under it).
fn from_the_palette(app: &mut App, action: Action) {
    feed(app, ":");
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))));
    app.apply_modal_result(ModalResult { actions: vec![action], close: true, usage: None });
    app.capture_undo();
    draw(app);
}

/// Final review C-1: the palette pops before its Undo runs, so the parked
/// session must be live again by then, or the engine is skipped, the gate
/// records nothing, and an older app step lands over the typed text.
#[test]
fn a_palette_undo_over_an_edited_url_steps_the_engine() {
    let mut app = url_line("abc");
    app.history.break_coalescing();
    app.editor.url = LineInput::new("abcdef");
    app.capture_undo();
    let before = snapshot(&app);
    feed(&mut app, "<CR>AXY<Esc>");
    assert_eq!(url(&app).0, "abcdefXY");
    from_the_palette(&mut app, Action::Undo);
    assert_eq!(url(&app).0, "abcdef", "the engine's step, not the app's");
    from_the_palette(&mut app, Action::Redo);
    assert_eq!(url(&app).0, "abcdefXY", "no typed text is lost");
    feed(&mut app, "<Esc>u");
    let after = snapshot(&app);
    assert!(after == before, "undo did not restore exactly:\n{}", snapshot_diff(&before, &after));
}

/// C-1's body twin.
#[test]
fn a_palette_undo_over_an_edited_body_steps_the_engine() {
    let mut app = vim(Fixture::Body);
    app.history.break_coalescing();
    app.editor.set_body_text("[1]");
    app.capture_undo();
    let before = snapshot(&app);
    feed(&mut app, "x");
    assert_eq!(body(&app), "1]");
    from_the_palette(&mut app, Action::Undo);
    assert_eq!(body(&app), "[1]", "the engine's step, not the app's");
    from_the_palette(&mut app, Action::Redo);
    assert_eq!(body(&app), "1]", "no edit is lost");
    feed(&mut app, "<Esc>");
    assert_eq!(app.vim.glue.session(), FieldSession::None);
    feed(&mut app, "u");
    let after = snapshot(&app);
    assert!(
        after.same_but_field(&before),
        "undo did not restore exactly:\n{}",
        snapshot_diff(&before, &after)
    );
}

/// Final review M-1: a declined trailing ctrl+r is the app's Redo only when
/// the engine was in idle Normal before it. In Visual, or with an operator
/// pending, Vim does nothing.
#[test]
fn a_declined_ctrl_r_outside_idle_normal_is_not_an_app_redo() {
    for keys in ["v<C-r>", "d<C-r>"] {
        let mut app = url_line("abc");
        app.history.break_coalescing();
        app.editor.url = LineInput::new("abcdef");
        app.capture_undo();
        feed(&mut app, "u");
        assert_eq!(url(&app).0, "abc", "the app step undone");
        feed(&mut app, "<CR>");
        feed(&mut app, keys);
        assert_eq!(url(&app).0, "abc", "{keys}: nothing redid");
        assert_eq!(app.history.redo_len(), 1, "{keys}: the redo is still there");
    }
}

/// M-1's guard: a count before `u` still goes to the app history, repeated
/// (spec §3.7).
#[test]
fn a_counted_u_with_nothing_to_undo_in_the_field_steps_the_app_history() {
    let mut app = url_line("a");
    for text in ["ab", "abc"] {
        app.history.break_coalescing();
        app.editor.url = LineInput::new(text);
        app.capture_undo();
    }
    feed(&mut app, "<CR>2u");
    assert_eq!(url(&app).0, "a");
}

/// Final review I-1: shift+Tab in the vim body's Insert never runs the
/// body component's dedent, which would edit outside the engine and wipe
/// the body's engine history.
#[test]
fn shift_tab_in_the_vim_body_never_dedents() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jxi");
    assert_eq!(body(&app), "{\n \"a\": 1\n}");
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    app.capture_undo();
    draw(&mut app);
    assert_eq!(body(&app), "{\n \"a\": 1\n}", "not dedented");
    assert!(app.vim.glue.can_undo(EngineKind::Body), "the engine's history survives");
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
    assert_eq!(app.vim.glue.session(), FieldSession::None, "the body closed");
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
    assert!(
        app.toasts.messages().iter().any(|m| m.contains("not supported")),
        "{:?}",
        app.toasts.messages()
    );
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
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
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
    assert_eq!(app.vim.glue.session(), FieldSession::None);
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

/// Plan M3a Deviation 6: the body's alt+←/→ word jump is arrows-only; and
/// vim's keymap binds no alt+←/→ (plan M4a Deviation 12: `gT`/`gt`), so the
/// chord does nothing in the vim body.
#[test]
fn alt_arrows_do_nothing_in_the_vim_body() {
    let mut app = vim(Fixture::Body);
    let before = caret(&app);
    for code in [KeyCode::Right, KeyCode::Left] {
        app.handle_key(KeyEvent::new(code, KeyModifiers::ALT));
        assert_eq!(app.editor.active_tab, EditorTab::Body, "{code:?}: no tab cycle");
        assert_eq!(body(&app), BODY, "{code:?}");
        assert_eq!(caret(&app), before, "{code:?}: no word jump");
    }
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
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body), "back in by keyboard");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step from the first visit");
}

/// Code review: a bracketed paste is an event of its own, so the session
/// catches up first. A body replaced since (the `$EDITOR` round-trip) is
/// rebased, and its old history never replays onto the new text.
#[test]
fn a_paste_after_the_body_was_replaced_rebases_first() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x");
    assert!(app.vim.glue.can_undo(EngineKind::Body));
    app.editor.set_body_text("[1]");
    app.paste_text("Z");
    feed(&mut app, "u");
    assert_eq!(body(&app), "[1]", "u takes the paste back");
    assert!(!app.vim.glue.can_undo(EngineKind::Body), "the history of the old text is gone");
}

/// Code review: every scratch has the same (no) slug, so a fresh scratch
/// must not inherit the last one's body history, even when the two bodies
/// read the same.
#[test]
fn a_fresh_scratch_does_not_inherit_the_bodys_history() {
    let on_body = |app: &mut App| {
        app.editor.method = postui_core::model::Method::Post;
        app.editor.active_tab = EditorTab::Body;
        app.editor.preferred_tab = EditorTab::Body;
        app.focus = PaneId::Editor;
        app.editor.sub_focus = SubFocus::Content;
        draw(app);
    };
    let mut app = App::new_for_test();
    assert_eq!(app.editor.slug, None, "a scratch");
    app.use_profile(KeyProfile::Vim);
    on_body(&mut app);
    feed(&mut app, "ihello<Esc>u");
    assert_eq!(body(&app), "");
    assert!(app.vim.glue.can_redo(EngineKind::Body));
    feed(&mut app, "<Esc>");
    assert_eq!(app.vim.glue.session(), FieldSession::None, "the body closed");
    app.land(Landing {
        open: Some(crate::undo::Open::Scratch { buffer: None }),
        ..Landing::default()
    });
    on_body(&mut app);
    feed(&mut app, "0");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    assert!(!app.vim.glue.can_redo(EngineKind::Body), "the old scratch's redo is gone");
}

/// Code review (final review I-1 generalized): a key keys.toml binds that
/// the engine declines never reaches the body's own editor, which would
/// select or edit outside the engine; the keymap's action runs instead.
#[test]
fn a_bound_key_the_engine_declines_skips_the_body_editor() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x");
    app.keymap.bind(KeyCombo::parse("shift+right").unwrap(), Action::OpenPalette);
    let before = (body(&app), caret(&app));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert!(matches!(app.modals.top(), Some(Modal::Palette(_))), "the keymap's action ran");
    assert_eq!((body(&app), caret(&app)), before);
    assert!(app.editor.body.selection.is_none(), "no GUI selection under the engine");
    assert!(app.vim.glue.can_undo(EngineKind::Body), "the engine's history survives");
}

/// Code review: a modal raised outside a key (an async result) parks the
/// body at the main loop's catch-up, so its search prompt stops drawing
/// before the next key.
#[test]
fn a_modal_raised_outside_a_key_parks_the_body_at_the_catch_up() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "/a");
    draw(&mut app);
    assert!(app.editor.body_search.prompt.is_some());
    app.update(Action::OpenPalette);
    app.vim_catch_up();
    draw(&mut app);
    assert_eq!(app.vim.glue.session(), FieldSession::Parked(FieldId::Body));
    assert!(app.editor.body_search.prompt.is_none(), "no prompt drawn under the modal");
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

/// Spec 2026-10-15 R11: vim `>>` steps by a 4-space body's width, still
/// does after the body's text is replaced under the session (its engine
/// state resets, as after `$EDITOR`), and a tab body keeps 2.
#[test]
fn shifts_follow_a_space_bodys_width() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("{\n    \"a\": 1\n}");
    app.editor.mark_saved();
    feed(&mut app, "gg>>");
    assert_eq!(body(&app), "    {\n    \"a\": 1\n}");
    app.editor.set_body_text("{\n    \"b\": 1\n}");
    feed(&mut app, "gg>>");
    assert_eq!(body(&app), "    {\n    \"b\": 1\n}", "a replaced body's fresh state");
    app.editor.set_body_text("{\n\t\"c\": 1\n}");
    app.editor.mark_saved();
    feed(&mut app, "gg>>");
    assert_eq!(body(&app), "  {\n\t\"c\": 1\n}", "a tab body keeps 2");
}

/// Review round 4: `>>` never feeds the next one's width, so repeated
/// shifts of a body's only indented line step 2, 4, 6, as Vim's do.
#[test]
fn repeated_shifts_step_by_a_fixed_width() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("a\n  b");
    app.editor.mark_saved();
    feed(&mut app, "G>>>>");
    assert_eq!(body(&app), "a\n      b");
}

/// R11: one-line fields keep `shiftwidth=2` whatever the body uses.
#[test]
fn the_url_line_shifts_by_two_under_a_four_space_body() {
    let mut app = url_line("x");
    app.editor.set_body_text("{\n    \"a\": 1\n}");
    feed(&mut app, "<CR>>>");
    assert_eq!(url(&app).0, "  x");
}

/// Spec 2026-10-15 R10: Format under a vim session writes the body's indent
/// too.
#[test]
fn a_format_with_the_body_open_follows_its_indent() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("{\n\t\"a\": [1]\n}");
    app.editor.mark_saved();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
    assert_eq!(body(&app), "{\n\t\"a\": [\n\t\t1\n\t]\n}");
}

/// Final review triage (T6): a clear with the body open is one engine
/// step, as a format is.
#[test]
fn a_body_clear_with_the_body_open_is_one_engine_step() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "0");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    let steps = app.history.undo_len();
    app.update(Action::BodyClear);
    assert_eq!(body(&app), "");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "u takes the clear back");
    assert!(app.vim.glue.can_redo(EngineKind::Body), "the engine's step, not the app's");
    assert_eq!(app.history.undo_len(), steps, "no app step was spent");
}

/// Final review M-3: replacing the body drops the engine's Visual paint
/// with it, so a stale span is never drawn over the new text.
#[test]
fn replacing_the_body_drops_its_visual_paint() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jV");
    assert!(app.editor.body_visual.is_some());
    app.editor.set_body_text("[]");
    assert_eq!(app.editor.body_visual, None);
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
    assert!(app.vim.glue.can_undo(EngineKind::Body));
    app.update(Action::ForceCreateRequest("other".into()));
    feed(&mut app, "<Esc>");
    assert!(!app.vim.glue.can_undo(EngineKind::Body));
}

/// Amendment C2: the body's state follows the open request's slug when a
/// rename changes it in place. The edit is from an earlier visit, so the
/// rename's `begin_op` has no live session edits to flush.
#[test]
fn renaming_the_open_request_keeps_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    let from = app.editor.slug.clone().expect("the fixture's request is saved");
    app.update(Action::RenameRequest { from: from.clone(), to: "renamed".into() });
    assert_ne!(app.editor.slug.as_deref(), Some(from.as_str()), "the editor follows the new slug");
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step, not the app's rename");
}

/// Final review triage (T6): a space rename re-keys the open request in
/// place too (amendment C2's extension), and the body's state follows.
#[test]
fn renaming_the_open_requests_space_keeps_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    let slug = app.editor.slug.clone().expect("the fixture's request is saved");
    let (space, _) = slug.split_once('/').expect("a request lives in a space");
    app.update(Action::RenameSpace { from: space.into(), to: "renamed".into() });
    assert!(
        app.toasts.messages().iter().any(|m| m.contains("Renamed space")),
        "{:?}",
        app.toasts.messages()
    );
    assert_ne!(app.editor.slug.as_deref(), Some(slug.as_str()), "the editor follows the new slug");
    let steps = app.history.undo_len();
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "the engine's step from the earlier visit");
    assert_eq!(app.history.undo_len(), steps, "not the app's step for the same edit");
}

/// Amendment C2 (controller ruling 2): an undo of the rename re-keys the
/// open request in place too, and the body's state follows it back.
#[test]
fn undoing_a_rename_keeps_the_bodys_history() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "x<Esc>jj");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
    let from = app.editor.slug.clone().expect("the fixture's request is saved");
    app.update(Action::RenameRequest { from: from.clone(), to: "renamed".into() });
    feed(&mut app, "<Esc>");
    assert_eq!(app.vim.glue.session(), FieldSession::None, "the body closed: `u` is the app's");
    feed(&mut app, "u");
    assert_eq!(app.editor.slug.as_deref(), Some(from.as_str()), "the rename was undone in place");
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    draw(&mut app);
    let steps = app.history.undo_len();
    feed(&mut app, "u");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
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
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body));
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
    assert_eq!(
        app.vim.glue.session(),
        FieldSession::Live(FieldId::Url),
        "the click carried the session"
    );
    let text = body(&app);
    assert!(
        !text.split('\n').any(|l| !l.is_empty() && l.trim().is_empty()),
        "a whitespace-only line is left: {text:?}"
    );
    feed(&mut app, "<Esc><Esc>j");
    assert_eq!(app.vim.glue.session(), FieldSession::Live(FieldId::Body), "back in by keyboard");
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
    assert_eq!(app.vim.glue.session(), FieldSession::None);
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

/// The user's ruling (final review T7): a paste in Visual replaces the
/// selection, as one engine step, and ends in Normal on the last char put.
#[test]
fn a_paste_in_visual_replaces_the_selection() {
    let mut app = url_line("abcdef");
    feed(&mut app, "<CR>0llvl");
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    app.paste_text("X\nY");
    assert_eq!(url(&app), ("abX Yef", 4), "flattened, the caret on the last char put");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "abcdef", "one undo step");
}

/// The same for a Visual the mouse made.
#[test]
fn a_paste_over_a_mouse_selection_replaces_it() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = url_line("https://x.test/ab");
    feed(&mut app, "<CR>");
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    let at = |kind, col| MouseEvent {
        kind,
        column: area.x + col,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 2));
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 7));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 7));
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    app.paste_text("XY");
    assert_eq!(url(&app), ("htXYx.test/ab", 3));
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "https://x.test/ab");
}

#[test]
fn a_body_paste_in_charwise_visual_replaces_the_selection() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jwvll");
    app.paste_text("x\ny");
    assert_eq!(body(&app), "{\n  x\ny: 1\n}");
    assert_eq!(caret(&app), (2, 0), "on the last char put");
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "one undo step");
}

#[test]
fn a_body_paste_in_linewise_visual_replaces_the_lines() {
    let mut app = vim(Fixture::Body);
    feed(&mut app, "jV");
    assert_eq!(app.mode_indicator(), Some("V-LINE"));
    app.paste_text("XY");
    assert_eq!(body(&app), "{\nXY\n}", "the line break after the lines stays");
    assert_eq!(caret(&app), (1, 1));
    feed(&mut app, "u");
    assert_eq!(body(&app), BODY, "one undo step");
}

/// A picked token replaces a Visual selection the same way.
#[test]
fn a_picked_token_in_visual_replaces_the_selection() {
    let mut app = url_line("abcdef");
    feed(&mut app, "<CR>0llvl");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("ab{{v}}ef", 6));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "abcdef", "one engine step");
}

/// #149: a picked `{{token}}` with the caret in a token
/// takes that token's place, as one engine step: from Normal the caret
/// lands on its last char (Vim's paste over a selection), from Insert
/// after it.
#[test]
fn a_picked_token_replaces_the_token_the_caret_is_in() {
    let mut app = url_line("a{{old}}b");
    feed(&mut app, "<CR>0lll");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("a{{v}}b", 5));
    assert_eq!(app.mode_indicator(), Some("NORMAL"));
    feed(&mut app, "u");
    assert_eq!(url(&app).0, "a{{old}}b", "one engine step");

    let mut app = url_line("a{{old}}b");
    feed(&mut app, "<CR>0llli");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("a{{v}}b", 6));
    assert_eq!(app.mode_indicator(), Some("INSERT"));

    // Insert's caret before the first brace is in front of the token.
    let mut app = url_line("{{old}}");
    feed(&mut app, "<CR>0i");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("{{v}}{{old}}", 5));

    // Insert ctrl+o after a token: the token goes in where Insert resumes,
    // after the token, not over it.
    let mut app = url_line("a{{old}}");
    feed(&mut app, "<CR>A<C-o>");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app).0, "a{{old}}{{v}}");

    // Normal on the last brace: the put goes after it, outside the token.
    let mut app = url_line("a{{old}}");
    feed(&mut app, "<CR>$");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app).0, "a{{old}}{{v}}");
}

/// #149: a Visual selection whose caret ends in a token is still what a
/// picked token replaces, as a paste over it.
#[test]
fn a_visual_selection_wins_over_the_carets_token() {
    let mut app = url_line("ab{{old}}");
    feed(&mut app, "<CR>0vlll");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app).0, "{{v}}old}}");
}

/// An app whose clipboard writes to a file, and that file.
fn with_clipboard_file(mut app: App) -> (App, tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let cmd = format!("cat > {}", out.to_string_lossy());
    app.set_clipboard_for_test(crate::clipboard::Clipboard::new_for_test(Some(cmd), 65536));
    (app, dir, out)
}

fn ctrl_c(app: &mut App) {
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
}

/// The user's decision on final review I-2: in vim a mouse selection
/// becomes Visual, and ctrl+c copies it rather than quitting.
#[test]
fn ctrl_c_copies_a_mouse_selection_in_an_open_url() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (mut app, _dir, out) = with_clipboard_file(url_line("https://x.test/ab"));
    feed(&mut app, "<CR>");
    let area = app.editor.last_url_text_area.expect("the URL line is drawn");
    let at = |kind, col| MouseEvent {
        kind,
        column: area.x + col,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 2));
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 7));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 7));
    assert_eq!(app.mode_indicator(), Some("VISUAL"));
    ctrl_c(&mut app);
    assert!(
        !app.should_quit && app.modals.is_empty(),
        "copy, not quit or its unsaved-changes gate"
    );
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "tps://");
}

#[test]
fn ctrl_c_copies_a_keyboard_visual_in_the_url() {
    let (mut app, _dir, out) = with_clipboard_file(url_line("abcdef"));
    feed(&mut app, "<CR>0lvll");
    ctrl_c(&mut app);
    assert!(!app.should_quit && app.modals.is_empty());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "bcd");
    assert_eq!(app.mode_indicator(), Some("VISUAL"), "the selection stays, as a GUI copy keeps it");
}

#[test]
fn ctrl_c_copies_the_bodys_visual() {
    let (mut app, _dir, out) = with_clipboard_file(vim(Fixture::Body));
    feed(&mut app, "jV");
    ctrl_c(&mut app);
    assert!(!app.should_quit && app.modals.is_empty());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "  \"a\": 1");
}

/// Spec §3.7: a picked token is an edit outside the key path.
#[test]
fn a_picked_token_goes_in_at_the_insert_caret() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>A");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("ab{{v}}", 7));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
}

/// Insert ctrl+o is still typing: the token goes in where Insert would
/// type it, not after the char under the ctrl+o caret.
#[test]
fn a_picked_token_after_insert_ctrl_o_goes_in_at_the_insert_caret() {
    let mut app = url_line("abc");
    feed(&mut app, "<CR>$i<C-o>");
    assert_eq!(app.mode_indicator(), Some("(insert)"));
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
    assert_eq!(url(&app), ("ab{{v}}c", 7));
    assert_eq!(app.mode_indicator(), Some("INSERT"));
}

#[test]
fn a_picked_token_in_normal_goes_after_the_caret() {
    let mut app = url_line("ab");
    feed(&mut app, "<CR>");
    app.update(Action::InsertVarText { name: "v".into(), completing: false });
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
    assert_eq!(app.vim.glue.session(), FieldSession::Parked(FieldId::Url));
    assert_eq!(url(&app).0, "ab{{");
}

use ratatui::style::Modifier;

pub(super) fn draw_buffer(app: &mut App) -> ratatui::buffer::Buffer {
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
    (area.x..area.right())
        .filter(|&x| buf[(x, area.y)].symbol() == c)
        .map(|x| buf[(x, area.y)].modifier)
        .collect()
}

/// Spec §5.6: the prompt is drawn on the body pane's bottom row, which
/// the vim profile keeps for it, open or not, as Vim keeps its command
/// line (#129): the body stays the rows above it.
#[test]
fn the_search_prompt_draws_on_the_bodys_bottom_row() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("foo bar foo");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    assert_eq!(row_text(&buf, area, area.bottom()).trim(), "", "a blank row below the body");
    feed(&mut app, "/fo");
    let buf = draw_buffer(&mut app);
    assert_eq!(app.editor.last_body_area, Some(area), "the body keeps its size");
    let bottom = row_text(&buf, area, area.bottom());
    assert!(bottom.starts_with("/fo"), "{bottom:?}");
    feed(&mut app, "<Esc>");
    let buf = draw_buffer(&mut app);
    assert_eq!(app.editor.last_body_area, Some(area), "and after Esc");
    assert_eq!(row_text(&buf, area, area.bottom()).trim(), "", "Esc closes the prompt");
}

/// #129: the arrows profile keeps no row for the vim prompt: its body
/// has the pane's rows, one more than the vim profile's.
#[test]
fn the_arrows_body_keeps_no_prompt_row() {
    let mut app = vim(Fixture::Body);
    let area = app.editor.last_body_area.unwrap();
    app.use_profile(KeyProfile::Arrows);
    draw(&mut app);
    let arrows = app.editor.last_body_area.unwrap();
    assert_eq!(arrows, ratatui::layout::Rect { height: area.height + 1, ..area });
}

/// #129, the task's steps: `L` puts the caret on the body's last row, and
/// `/` shows it there above the prompt, nothing moved. Esc leaves the
/// view as it was. The engine's window is the body's rows throughout.
#[test]
fn l_then_slash_keeps_the_carets_line_in_view() {
    let mut app = vim(Fixture::Body);
    let lines: Vec<String> = (0..120).map(|i| format!("row{i}")).collect();
    app.editor.set_body_text(&lines.join("\n"));
    draw(&mut app);
    feed(&mut app, "L");
    let before = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let h = usize::from(area.height);
    let caret = app.editor.body.cursor.row;
    assert_eq!(caret, h - 1, "L: the last row");
    assert_eq!(app.view_ctx(FieldId::Body).viewport_rows, Some(h));
    let body = |buf: &ratatui::buffer::Buffer| -> Vec<String> {
        (area.y..area.bottom()).map(|y| row_text(buf, area, y)).collect()
    };
    // One key and one frame, no settling draw after.
    for ev in crate::keyroute::notation::events("/") {
        app.handle_key(ev);
    }
    let open = draw_buffer(&mut app);
    assert_eq!(app.editor.last_body_area, Some(area));
    assert_eq!(body(&open), body(&before), "nothing shifts");
    let last = row_text(&open, area, area.bottom() - 1);
    assert!(last.trim_end().ends_with(&format!("row{caret}")), "{last:?}");
    assert!(row_text(&open, area, area.bottom()).starts_with('/'));
    assert_eq!(app.view_ctx(FieldId::Body).viewport_rows, Some(h));
    feed(&mut app, "<Esc>");
    let closed = draw_buffer(&mut app);
    assert_eq!(app.editor.body.viewport_offset().1, 0);
    assert_eq!(body(&closed), body(&before), "Esc leaves the view as it was");
}

/// #129: the prompt's row is not the body's, open or not, so a click on
/// it doesn't move the body's caret to a line the body doesn't show.
#[test]
fn a_click_on_the_search_prompt_leaves_the_body_caret() {
    let mut app = vim(Fixture::Body);
    let lines: Vec<String> = (0..120).map(|i| format!("row{i}")).collect();
    app.editor.set_body_text(&lines.join("\n"));
    draw(&mut app);
    let caret = app.editor.body.cursor;
    let area = app.editor.last_body_area.unwrap();
    assert!(!app.hits.rects_of(&Hit::BodyEditor).iter().any(|r| r.bottom() > area.bottom()));
    click_at(&mut app, area.x + 8, area.bottom());
    assert_eq!(app.editor.body.cursor, caret, "the blank row");
    feed(&mut app, "/x");
    assert!(!app.hits.rects_of(&Hit::BodyEditor).iter().any(|r| r.bottom() > area.bottom()));
    click_at(&mut app, area.x + 8, area.bottom());
    assert_eq!(app.editor.body.cursor, caret, "the open prompt");
}

/// #129: the bar measures the body above the prompt's row, as the thumb
/// drag's spec does, and stays as it is when the prompt opens.
#[test]
fn the_search_prompt_leaves_the_scrollbar() {
    let mut app = vim(Fixture::Body);
    let h = usize::from(app.editor.last_body_area.expect("the body is drawn").height);
    let thumb = |app: &App| app.hits.rect_of(&Hit::ScrollbarThumb(PaneId::Editor));
    app.editor.set_body_text(&vec!["x"; h].join("\n"));
    draw(&mut app);
    assert_eq!(thumb(&app), None, "the body's rows fit");
    app.editor.set_body_text(&vec!["x"; h + 1].join("\n"));
    draw(&mut app);
    let bar = thumb(&app).expect("one line more than the body shows");
    let area = app.editor.last_body_area.unwrap();
    let spec = app.editor.scrollbar_spec().unwrap();
    assert_eq!(crate::hit::thumb_geometry(&spec, area.height).1, bar.height);
    feed(&mut app, "/");
    assert_eq!(thumb(&app), Some(bar));
    assert_eq!(app.editor.last_body_area, Some(area));
}

/// #129: a two-row pane gives the body one row above the prompt's; the
/// frame after Enter still paints the match the search jumped to.
#[test]
fn a_one_row_body_paints_the_match_after_the_prompt() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    fn draw_at(app: &mut App, rows: u16) -> ratatui::buffer::Buffer {
        app.anims.finish_all();
        let mut terminal = Terminal::new(TestBackend::new(160, rows)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }
    let mut app = vim(Fixture::Body);
    let mut lines: Vec<String> = (0..120).map(|i| format!("row{i}")).collect();
    lines[100] = "needle".into();
    app.editor.set_body_text(&lines.join("\n"));
    let rows = (8..48)
        .find(|&h| {
            draw_at(&mut app, h);
            app.editor.last_body_area.map(|a| a.height) == Some(1)
        })
        .expect("a height with a one-row body");
    let mut buf = draw_at(&mut app, rows);
    for ev in crate::keyroute::notation::events("/needle<CR>") {
        app.handle_key(ev);
        buf = draw_at(&mut app, rows);
    }
    let area = app.editor.last_body_area.unwrap();
    assert_eq!(area.height, 1);
    assert!(row_text(&buf, area, area.y).contains("needle"));
    // The second letter: the caret's own cell is REVERSED regardless.
    let x = (area.x..area.right()).find(|&x| buf[(x, area.y)].symbol() == "e").unwrap();
    assert!(buf[(x, area.y)].modifier.contains(Modifier::REVERSED), "the match is painted");
}

/// The prompt's caret stands past the cells its text paints: `❤️` (a
/// heart and its presentation selector) takes two.
#[test]
fn the_search_prompts_caret_follows_the_painted_cells() {
    let mut app = vim(Fixture::Body);
    app.editor.set_body_text("foo");
    feed(&mut app, "/\u{2764}\u{fe0f}x");
    let buf = draw_buffer(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let y = area.bottom(); // the prompt's row, below the body
    let reversed: Vec<u16> = (area.x..area.right())
        .filter(|&x| buf[(x, y)].modifier.contains(Modifier::REVERSED))
        .collect();
    assert_eq!(reversed, [area.x + 4], "after `/`, the heart's two cells and `x`");
}

/// Final review M-4: a `{{token}}` on the body's bottom row keeps its hit
/// there as the prompt opens below it (#129), and the prompt's row has
/// none a click could find.
#[test]
fn the_search_prompt_row_has_no_token_hits() {
    let mut app = vim(Fixture::Body);
    let h = usize::from(app.editor.last_body_area.expect("the body is drawn").height);
    let mut lines = vec!["x"; h - 1];
    lines.push("{{v}}");
    lines.push("{{v}}"); // one line below the body
    app.editor.set_body_text(&lines.join("\n"));
    draw(&mut app);
    let area = app.editor.last_body_area.unwrap();
    let rows = |app: &App| -> Vec<u16> {
        app.hits.rects_of(&Hit::VarToken("v".into())).iter().map(|r| r.y).collect()
    };
    assert_eq!(rows(&app), [area.bottom() - 1], "the body's bottom row only");
    feed(&mut app, "/x");
    assert!(app.editor.body_search.prompt.is_some());
    assert_eq!(rows(&app), [area.bottom() - 1], "the same, none on the prompt");
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
    assert!(
        fs.iter().all(|m| m.contains(Modifier::REVERSED)),
        "the highlight stays after the search"
    );
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
    assert!(
        cells_of(&buf, area, "o")[0].contains(Modifier::REVERSED),
        "the highlight survives the rename"
    );
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
    assert!(
        buf[(x, hit)].modifier.contains(Modifier::REVERSED),
        "painted in the frame that scrolled"
    );
}
