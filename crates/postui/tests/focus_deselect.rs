//! Deselection and focus-visibility acceptance: every editor input can be
//! left with Enter, Esc, or a click elsewhere, and the screen only ever
//! shows focus styling for the control that actually receives keys.

use postui::app::App;
use postui::components::editor::{EditorTab, SubFocus};
use postui::hit::Hit;
use postui::keys::Keymap;
use postui::layout::PaneId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

fn render(app: &mut App) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    terminal
}

fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn type_text(app: &mut App, keymap: &Keymap, text: &str) {
    for c in text.chars() {
        app.handle_key(keymap, plain(c));
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

/// Re-renders `app` (so `app.hits` is fresh), looks up `hit`'s rect, and
/// sends a left-button Down at its center.
fn click(app: &mut App, hit: Hit) {
    render(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    app.handle_mouse(left_down(r.x + r.width / 2, r.y + r.height / 2));
}

// --- deselection ---------------------------------------------------------

#[test]
fn enter_deselects_the_url_input() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    click(&mut app, Hit::UrlBar);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    type_text(&mut app, &keymap, "https://x");
    app.handle_key(&keymap, key(KeyCode::Enter));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::None,
        "Enter blurs the URL input"
    );
    assert_eq!(
        app.editor.url.text(),
        "https://x",
        "Enter commits, not edits"
    );
}

#[test]
fn esc_deselects_the_url_input() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    click(&mut app, Hit::UrlBar);
    app.handle_key(&keymap, key(KeyCode::Esc));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::None,
        "Esc blurs the URL input"
    );
}

#[test]
fn clicking_another_pane_deselects_the_url_input() {
    let mut app = App::new_for_test();
    click(&mut app, Hit::UrlBar);
    click(&mut app, Hit::Pane(PaneId::Response));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::None,
        "a click on the response pane background blurs the URL input"
    );
    // And the input is re-enterable afterwards.
    click(&mut app, Hit::UrlBar);
    assert_eq!(app.editor.sub_focus, SubFocus::Url);

    click(&mut app, Hit::Pane(PaneId::Sidebar));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::None,
        "a click on the sidebar background blurs the URL input"
    );
}

#[test]
fn esc_deselects_the_body_editor() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    app.focus = PaneId::Editor;
    app.editor.active_tab = EditorTab::Body;
    app.editor.sub_focus = SubFocus::Content;
    app.handle_key(&keymap, key(KeyCode::Esc));
    assert_eq!(
        app.editor.sub_focus,
        SubFocus::None,
        "Esc blurs the body editor rather than jumping to the URL line"
    );
}

#[test]
fn typing_while_blurred_reaches_no_input() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    click(&mut app, Hit::UrlBar);
    type_text(&mut app, &keymap, "abc");
    app.handle_key(&keymap, key(KeyCode::Enter));
    type_text(&mut app, &keymap, "xyz");
    assert_eq!(
        app.editor.url.text(),
        "abc",
        "keys after blur don't land in the URL input"
    );
}

// --- focus visibility ----------------------------------------------------

/// Whether any cell inside `pane`'s rect shows the focused URL well's
/// lifted fill color (the URL input's focus indicator).
fn has_lifted_url_fill(term: &Terminal<TestBackend>, app: &App, pane: PaneId) -> bool {
    let area = term.backend().buffer().area;
    let layout = postui::layout::compute_layout(area, 0.0, 0.0, 0.5);
    let r = match pane {
        PaneId::Sidebar => layout.sidebar,
        PaneId::Editor => layout.editor,
        PaneId::Response => layout.response,
    };
    let lifted = postui::theme::lift_color(app.theme.control, 0.12);
    let buf = term.backend().buffer();
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            if buf[(x, y)].bg == lifted {
                return true;
            }
        }
    }
    false
}

#[test]
fn url_focus_lift_paints_only_when_the_editor_pane_is_focused() {
    let mut app = App::new_for_test();
    // Default state: sub_focus is Url but the Sidebar pane has focus — keys
    // do not reach the URL input, so it must not claim focus on screen.
    assert_eq!(app.editor.sub_focus, SubFocus::Url);
    assert_eq!(app.focus, PaneId::Sidebar);
    let term = render(&mut app);
    assert!(
        !has_lifted_url_fill(&term, &app, PaneId::Editor),
        "no lifted URL fill while the sidebar pane has focus"
    );

    app.focus = PaneId::Editor;
    let term = render(&mut app);
    assert!(
        has_lifted_url_fill(&term, &app, PaneId::Editor),
        "the URL well lifts when the editor pane has focus and sub-focus is Url"
    );

    app.focus = PaneId::Response;
    let term = render(&mut app);
    assert!(
        !has_lifted_url_fill(&term, &app, PaneId::Editor),
        "tabbing away drops the URL well back to its resting fill"
    );
}

/// The focused pane's left padding column carries a `▌` accent bar; the
/// other panes' columns don't.
fn pane_has_focus_bar(term: &Terminal<TestBackend>, app: &App, pane: PaneId) -> bool {
    let area = term.backend().buffer().area;
    let layout = postui::layout::compute_layout(area, 0.0, 0.0, 0.5);
    let r = match pane {
        PaneId::Sidebar => layout.sidebar,
        PaneId::Editor => layout.editor,
        PaneId::Response => layout.response,
    };
    let buf = term.backend().buffer();
    (r.y..r.y + r.height).all(|y| {
        let cell = &buf[(r.x, y)];
        cell.symbol() == "▌" && cell.fg == app.theme.focus_ring
    })
}

#[test]
fn focused_pane_shows_a_left_accent_bar() {
    let mut app = App::new_for_test();
    let keymap = Keymap::default_bindings();
    for pane in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
        assert_eq!(app.focus, pane);
        let term = render(&mut app);
        for p in [PaneId::Sidebar, PaneId::Editor, PaneId::Response] {
            assert_eq!(
                pane_has_focus_bar(&term, &app, p),
                p == pane,
                "exactly the focused pane ({pane:?}) shows the accent bar; checked {p:?}"
            );
        }
        app.handle_key(&keymap, key(KeyCode::Tab));
    }
}

/// The bar keeps the background of whatever it is drawn over, so pane
/// content must stay out of the left padding column: a control filling it
/// with the accent color would swallow the bar (accent-on-accent) and read
/// as bleeding flush into the pane edge — the "+ New request" button did
/// exactly that.
#[test]
fn focus_bar_stays_visible_over_the_new_request_button() {
    let mut app = App::new_for_test();
    assert_eq!(app.focus, PaneId::Sidebar);
    let term = render(&mut app);
    let layout = postui::layout::compute_layout(term.backend().buffer().area, 0.0, 0.0, 0.5);
    let r = layout.sidebar;
    let buf = term.backend().buffer();
    for y in r.y..r.y + r.height {
        let cell = &buf[(r.x, y)];
        assert_eq!(cell.symbol(), "▌", "bar glyph intact at row {y}");
        assert_ne!(
            cell.bg, app.theme.focus_ring,
            "bar cell at row {y} keeps a contrasting background — pane content must not fill the padding column with the accent color"
        );
    }
}

/// The sidebar reserves the same left column for the selected request's
/// accent marker. The pane focus bar composes with the marker's `█` text
/// The row list shares the New-request button's 1-column inset, so column
/// 0 is the pane focus bar's lane alone: with the pane focused the bar
/// runs unbroken top to bottom, and the selected row's accent marker sits
/// one column in, inside its own pill — the two never share a cell.
#[test]
fn selected_request_marker_sits_beside_the_sidebar_focus_bar() {
    use postui::components::sidebar::Row;
    use postui_core::model::Method;

    let mut app = App::new_for_test();
    app.sidebar.rows.push(Row::Request {
        slug: "go".into(),
        name: "go".into(),
        depth: 0,
        broken: None,
        method: Some(Method::Get),
    });
    app.sidebar.selected = Some(0);
    // The accent marker tracks the OPEN request, not the browse cursor.
    app.sidebar.open_slug = Some("go".into());
    assert_eq!(app.focus, PaneId::Sidebar);
    let term = render(&mut app);
    let layout = postui::layout::compute_layout(term.backend().buffer().area, 0.0, 0.0, 0.5);
    let r = layout.sidebar;
    let buf = term.backend().buffer();
    for y in r.y..r.y + r.height {
        let cell = &buf[(r.x, y)];
        assert_eq!(
            cell.symbol(),
            "▌",
            "the bar column holds only the focus bar (row {y}) — the bar must run unbroken"
        );
        assert_eq!(cell.fg, app.theme.focus_ring);
    }
    let mut markers = 0;
    for y in r.y..r.y + r.height {
        let cell = &buf[(r.x + 1, y)];
        if cell.symbol() == "▌" && cell.fg == app.theme.accent {
            markers += 1;
        }
    }
    assert_eq!(
        markers, 1,
        "the selected row's accent marker shows once, one column in from the bar"
    );
}

/// Hovering a sidebar row must not disturb the bar column: the hover
/// pill's fill (and its pad caps composing with a neighboring selected
/// pill) previously leaked into column 0, leaving half-covered fill chips
/// beside the bar. Every bar cell keeps the pane's own surface behind it;
/// only the selection marker cell differs.
#[test]
fn hovering_a_row_leaves_the_sidebar_focus_bar_clean() {
    use postui::components::sidebar::Row;
    use postui_core::model::Method;

    let mut app = App::new_for_test();
    for slug in ["go", "test"] {
        app.sidebar.rows.push(Row::Request {
            slug: slug.into(),
            name: slug.into(),
            depth: 0,
            broken: None,
            method: Some(Method::Get),
        });
    }
    app.sidebar.selected = Some(0);
    app.hovered = Some(Hit::SidebarRow(1));
    assert_eq!(app.focus, PaneId::Sidebar);
    let term = render(&mut app);
    let layout = postui::layout::compute_layout(term.backend().buffer().area, 0.0, 0.0, 0.5);
    let r = layout.sidebar;
    let buf = term.backend().buffer();
    for y in r.y..r.y + r.height {
        let cell = &buf[(r.x, y)];
        if cell.symbol() == "█" {
            continue; // the selection marker fills its whole cell
        }
        assert_eq!(cell.symbol(), "▌", "bar glyph at row {y}");
        assert_eq!(
            cell.bg, app.theme.panel,
            "bar cell at row {y} sits on the pane surface — no hover fill may leak into the bar column"
        );
    }
}
