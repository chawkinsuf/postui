//! Stage-7 §7: inline `{{variable}}` highlighting and the value tooltip
//! (the user's #5 complaint — "I can't see what the current value of a
//! variable is when I'm in the request").
//!
//! Everything here goes through the real draw and the real `MouseEvent`s a
//! terminal delivers, so token registration, tinting, hover, the tooltip's
//! content and the click-through to the picker are exercised together.

use postui::action::Action;
use postui::app::App;
use postui::components::editor::{EditorTab, SubFocus};
use postui::components::modal::Modal;
use postui::hit::Hit;
use postui_core::model::Entry;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

const W: u16 = 120;
const H: u16 = 40;

fn draw(app: &mut App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(W, H)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn dump(app: &mut App) -> String {
    format!("{:?}", draw(app))
}

fn moved(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Moved,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

/// A project with one env-valued variable, one variable that only has a
/// declaration default, a secret, and a one-field group — every scope the
/// tooltip can name.
fn app_with_vars() -> App {
    let mut app = App::new_for_test();
    app.project
        .edit_variables(|_| {
            Ok("[base_url]\ndefault = \"http://fallback\"\n\n\
                [page]\ndefault = \"1\"\n\n\
                [api_key]\nsecret = true\n\n\
                [selectors.user]\nfields = [\"uid\"]\n"
                .to_string())
        })
        .unwrap();
    app.project
        .edit_env("qa", |_| {
            Ok("base_url = \"http://qa.test\"\n\n\
                [options.user.\"user 2\"]\nuid = \"1001\"\n"
                .to_string())
        })
        .unwrap();
    app.project.set_env(Some("qa".into()));
    app.project
        .set_secret("api_key", "sk-qa-999".into())
        .unwrap();
    app.update(Action::Render);
    app
}

fn set_url(app: &mut App, url: &str) {
    app.editor.url = postui::components::line_input::LineInput::new(url);
    app.update(Action::Render);
}

fn token_rect(app: &mut App, name: &str) -> Rect {
    draw(app);
    app.hits
        .rect_of(&Hit::VarToken(name.to_string()))
        .unwrap_or_else(|| panic!("no VarToken registered for {name}"))
}

/// Hovers the middle of `name`'s drawn span and returns its rect.
fn hover_token(app: &mut App, name: &str) -> Rect {
    let r = token_rect(app, name);
    app.handle_mouse(moved(r.x + 1, r.y));
    r
}

#[test]
fn a_url_token_registers_a_var_token_hit_inside_the_url_bar() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/x");
    draw(&mut app);

    let url_bar = app.hits.rect_of(&Hit::UrlBar).unwrap();
    let token = app
        .hits
        .rect_of(&Hit::VarToken("base_url".into()))
        .expect("the URL's token registers a hit");
    assert_eq!(token.width, 12, "the whole `{{{{base_url}}}}` span");
    assert_eq!(token.height, 1);
    assert!(
        token.x >= url_bar.x
            && token.right() <= url_bar.right()
            && token.y >= url_bar.y
            && token.bottom() <= url_bar.bottom(),
        "token {token:?} must sit inside the URL bar {url_bar:?}"
    );
    // Registered *over* the URL bar: a click resolves to the token.
    assert_eq!(
        app.hits.hit_at(token.x + 1, token.y),
        Some(&Hit::VarToken("base_url".into()))
    );
}

#[test]
fn resolved_tokens_are_tinted_and_unresolved_ones_render_in_the_error_color() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/{{nope}}");
    let buf = draw(&mut app);

    let good = app.hits.rect_of(&Hit::VarToken("base_url".into())).unwrap();
    let bad = app.hits.rect_of(&Hit::VarToken("nope".into())).unwrap();
    assert_eq!(buf[(good.x, good.y)].fg, app.theme.accent_edge_dark);
    assert_ne!(buf[(good.x, good.y)].fg, app.theme.error);
    for x in bad.x..bad.right() {
        assert_eq!(
            buf[(x, bad.y)].fg,
            app.theme.error,
            "every cell of an unresolved token is the error color"
        );
    }
    // The literal text between them keeps the ordinary text color.
    assert_eq!(buf[(good.right(), good.y)].fg, app.theme.text);
}

#[test]
fn hovering_a_token_shows_its_value_and_scope_and_leaving_drops_the_tooltip() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/x");
    hover_token(&mut app, "base_url");

    let tip = app.var_token_tip().expect("hovering makes a tip");
    assert_eq!(tip.name, "base_url");
    let frame = dump(&mut app);
    assert!(
        frame.contains("base_url = http://qa.test"),
        "the tooltip shows the value: {frame}"
    );
    assert!(
        frame.contains("env qa"),
        "...and the scope it came from: {frame}"
    );

    // Moving off the token drops it on the very next motion event.
    let away = app.hits.rect_of(&Hit::UrlBar).unwrap();
    app.handle_mouse(moved(away.right() - 1, away.y));
    assert!(app.var_token_tip().is_none(), "no lingering tooltip");
    let frame = dump(&mut app);
    assert!(!frame.contains("env qa"), "tooltip is gone: {frame}");
}

/// A press moves the pointer too. Without that, a terminal that reports no
/// motion (or a click that arrives with no `Moved` before it) left the last
/// hover's tooltip floating over the UI — covering the very controls the
/// next click aims at. Found by the stage-7 tmux sweep.
#[test]
fn a_click_elsewhere_drops_a_tooltip_even_with_no_motion_event() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/x");
    hover_token(&mut app, "base_url");
    assert!(app.var_token_tip().is_some());

    // No `Moved` in between: the press itself is the pointer's new home.
    let away = app.hits.rect_of(&Hit::EditorTab(0)).unwrap();
    app.handle_mouse(left_down(away.x + 1, away.y));
    assert!(
        app.var_token_tip().is_none(),
        "the tip belongs to where the pointer is now"
    );
    let frame = dump(&mut app);
    assert!(
        !frame.contains("env qa"),
        "and it is off the screen: {frame}"
    );
}

#[test]
fn the_scope_line_names_default_group_request_and_missing_secret() {
    let mut app = app_with_vars();

    // A declaration default (no env value).
    set_url(&mut app, "{{page}}");
    hover_token(&mut app, "page");
    let frame = dump(&mut app);
    assert!(
        frame.contains("page = 1") && frame.contains("default"),
        "{frame}"
    );

    // A group field, once its group has a selection.
    app.project.set_selection("user", "user 2");
    app.update(Action::Render);
    set_url(&mut app, "{{uid}}");
    hover_token(&mut app, "uid");
    let frame = dump(&mut app);
    assert!(frame.contains("uid = 1001"), "{frame}");
    assert!(
        frame.contains("selector user \u{2192} \"user 2\""),
        "the group and its selected option: {frame}"
    );

    // The request's own `[variables]` overlay outranks everything.
    app.editor.variables.insert(
        "base_url".into(),
        Entry {
            value: "http://local".into(),
            enabled: true,
        },
    );
    app.update(Action::Render);
    set_url(&mut app, "{{base_url}}");
    hover_token(&mut app, "base_url");
    let frame = dump(&mut app);
    assert!(
        frame.contains("base_url = http://local") && frame.contains("request var"),
        "{frame}"
    );
}

#[test]
fn a_group_field_with_no_selection_reads_as_needs_selection() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{uid}}");
    let buf = draw(&mut app);
    let r = app.hits.rect_of(&Hit::VarToken("uid".into())).unwrap();
    assert_eq!(
        buf[(r.x, r.y)].fg,
        app.theme.error,
        "an unselected group field cannot resolve"
    );

    hover_token(&mut app, "uid");
    let frame = dump(&mut app);
    assert!(frame.contains("needs selection"), "{frame}");
}

#[test]
fn a_secrets_tooltip_is_masked_and_never_reveals_the_value() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{api_key}}");
    hover_token(&mut app, "api_key");
    let frame = dump(&mut app);
    assert!(
        frame.contains("api_key = \u{25cf}"),
        "masked value: {frame}"
    );
    assert!(
        !frame.contains("sk-qa-999"),
        "a secret is never shown in the clear: {frame}"
    );

    // ...and a secret with no value for this env reads as missing.
    let mut app = app_with_vars();
    app.project.set_env(Some("qa".into()));
    app.project.edit_env("dev", |_| Ok(String::new())).unwrap();
    app.project.set_env(Some("dev".into()));
    app.update(Action::Render);
    set_url(&mut app, "{{api_key}}");
    hover_token(&mut app, "api_key");
    let frame = dump(&mut app);
    assert!(frame.contains("missing secret"), "{frame}");
    assert!(!frame.contains("sk-qa-999"), "{frame}");
}

#[test]
fn clicking_a_token_opens_the_var_picker_prefiltered_to_it() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/x");
    let r = token_rect(&mut app, "base_url");
    app.handle_mouse(left_down(r.x + 1, r.y));

    match app.modals.top() {
        Some(Modal::VarPicker(state)) => {
            assert_eq!(state.input(), "base_url", "the filter is seeded");
        }
        _ => panic!("expected a seeded var picker on top of the modal stack"),
    }
    let frame = dump(&mut app);
    assert!(frame.contains("base_url"), "{frame}");
    assert!(
        !frame.contains("api_key"),
        "the seed filters the other variables out: {frame}"
    );
}

#[test]
fn tokens_in_table_cells_are_tinted_and_hoverable_without_disturbing_the_table() {
    let mut app = app_with_vars();
    app.editor.params.insert(
        "q".into(),
        Entry {
            value: "{{base_url}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Params;
    app.editor.table.selected = Some(0);
    app.update(Action::Render);

    let buf = draw(&mut app);
    let r = app.hits.rect_of(&Hit::VarToken("base_url".into())).unwrap();
    let cell = app
        .hits
        .rect_of(&Hit::TableCell { row: 0, col: 1 })
        .unwrap();
    assert!(
        r.x >= cell.x && r.right() <= cell.right() && r.y == cell.y,
        "the token sits in the value cell: {r:?} vs {cell:?}"
    );
    assert_eq!(buf[(r.x, r.y)].fg, app.theme.accent_edge_dark);

    // Hovering the token neither blurs the table nor drops its selection...
    app.handle_mouse(moved(r.x + 1, r.y));
    assert_eq!(app.editor.table.selected, Some(0));
    assert!(app.var_token_tip().is_some());
    assert_eq!(
        app.hovered,
        Some(Hit::TableCell { row: 0, col: 1 }),
        "the cell under the token keeps the hover styling"
    );

    // ...and neither does clicking it (which opens the picker instead of
    // starting a cell edit).
    app.handle_mouse(left_down(r.x + 1, r.y));
    assert_eq!(app.editor.table.selected, Some(0));
    assert!(app.editor.table.editing.is_none(), "no cell edit started");
    assert!(matches!(app.modals.top(), Some(Modal::VarPicker(_))));
}

#[test]
fn a_cell_under_edit_keeps_its_caret_instead_of_registering_tokens() {
    let mut app = app_with_vars();
    app.editor.params.insert(
        "q".into(),
        Entry {
            value: "{{base_url}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Params;
    app.update(Action::Render);
    // Straight through the table's own click entry point: the cell's own
    // hit is covered by the token, which is the point of the next assert.
    app.editor
        .click_table_cell(0, postui::components::table_editor::Col::Value);
    assert!(app.editor.table.editing.is_some(), "the cell is under edit");

    draw(&mut app);
    assert!(
        app.hits
            .rect_of(&Hit::VarToken("base_url".into()))
            .is_none(),
        "the cell being typed into must stay a plain text field"
    );
}

#[test]
fn computed_header_rows_tint_only_the_unresolved_span() {
    let mut app = app_with_vars();
    app.editor.headers.insert(
        "X-Trace".into(),
        Entry {
            value: "{{base_url}}".into(),
            enabled: true,
        },
    );
    app.project
        .edit_variables(|doc| Ok(format!("{doc}\n[missing_one]\n")))
        .unwrap();
    // A project default header carrying one resolvable and one unresolvable
    // token: only the second may be tinted red.
    app.project.meta.default_headers.insert(
        "X-Auto".into(),
        Entry {
            value: "{{base_url}}/{{missing_one}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Headers;
    app.update(Action::Render);

    let buf = draw(&mut app);
    let good = app.hits.rect_of(&Hit::VarToken("base_url".into())).unwrap();
    let bad = app
        .hits
        .rect_of(&Hit::VarToken("missing_one".into()))
        .expect("the computed row's unresolved token registers");
    assert_eq!(
        buf[(bad.x, bad.y)].fg,
        app.theme.error,
        "the unresolved token is red"
    );
    assert_ne!(
        buf[(good.x, good.y)].fg,
        app.theme.error,
        "the resolved parts of the same value are not"
    );
}

#[test]
fn body_editor_tokens_are_tinted_and_hoverable() {
    let mut app = app_with_vars();
    app.editor.active_tab = EditorTab::Body;
    app.editor.set_body_text("{\"u\": \"{{base_url}}\"}");
    app.update(Action::Render);

    let buf = draw(&mut app);
    let r = app
        .hits
        .rect_of(&Hit::VarToken("base_url".into()))
        .expect("the body's token registers");
    assert_eq!(buf[(r.x, r.y)].fg, app.theme.accent_edge_dark);
    let body = app.hits.rect_of(&Hit::BodyEditor).unwrap();
    assert!(r.x >= body.x && r.right() <= body.right());

    app.handle_mouse(moved(r.x + 1, r.y));
    let frame = dump(&mut app);
    assert!(frame.contains("base_url = http://qa.test"), "{frame}");
}

#[test]
fn a_resting_caret_raises_the_same_tooltip_after_two_ticks() {
    let mut app = app_with_vars();
    set_url(&mut app, "{{base_url}}/x");
    app.editor.sub_focus = SubFocus::Url;
    app.editor.url.set_cursor(4);
    draw(&mut app);

    assert!(
        app.var_token_tip().is_none(),
        "not yet: the caret just landed"
    );
    app.update(Action::Tick);
    draw(&mut app);
    assert!(app.var_token_tip().is_none(), "no dwell yet is not resting");
    // The dwell is wall-clock (see `App::track_caret_token`), not
    // tick-counted -- real time is fine here, matching the sleep-based
    // settle precedent elsewhere in the app-level tests.
    std::thread::sleep(std::time::Duration::from_millis(250));
    app.update(Action::Tick);
    draw(&mut app);
    let tip = app
        .var_token_tip()
        .expect("resting past the dwell raises the tip");
    assert_eq!(tip.name, "base_url");
    let frame = dump(&mut app);
    assert!(frame.contains("base_url = http://qa.test"), "{frame}");

    // Moving the caret out of the token takes it away again.
    app.editor.url.set_cursor(13);
    app.update(Action::Tick);
    assert!(app.var_token_tip().is_none(), "the caret left the token");
}

/// Regression: the tooltip was anchored at the rect captured when the
/// pointer last moved, so a token that stopped being drawn under a resting
/// pointer (tab switch, scroll, layout change) left its tooltip behind.
#[test]
fn the_tooltip_leaves_with_its_token_even_if_the_pointer_never_moves() {
    let mut app = app_with_vars();
    app.editor.params.insert(
        "q".into(),
        Entry {
            value: "{{base_url}}".into(),
            enabled: true,
        },
    );
    app.editor.active_tab = EditorTab::Params;
    app.update(Action::Render);
    hover_token(&mut app, "base_url");
    assert!(app.var_token_tip().is_some());

    // The pointer stays exactly where it is; the token goes away.
    app.editor.active_tab = EditorTab::Body;
    app.update(Action::Render);
    draw(&mut app);
    assert!(
        app.var_token_tip().is_none(),
        "the tooltip must not outlive the token it describes"
    );
}
