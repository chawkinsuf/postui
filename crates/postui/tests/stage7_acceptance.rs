//! Stage-7 acceptance: right-click context menus (spec §5) and the
//! mouse-reachable "duplicate request" flow (spec §8). Everything here goes
//! through the same raw `MouseEvent`s the terminal delivers, so the tests
//! exercise hit registration, menu painting and click dispatch together
//! rather than poking `App` state directly.

use postui::action::Action;
use postui::app::App;
use postui::components::modal::{Modal, PromptKind};
use postui::components::sidebar::Row;
use postui::hit::Hit;
use postui::keys::{KeyCombo, Keymap};
use postui::layout::PaneId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn left_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

fn right_down(x: u16, y: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }
}

/// Re-renders so `app.hits` is fresh, then sends `make` at `hit`'s rect —
/// left edge + 2 columns, top row, which is inside every row-shaped hit.
fn press(app: &mut App, hit: Hit, make: fn(u16, u16) -> MouseEvent) -> ratatui::layout::Rect {
    render(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    app.handle_mouse(make(r.x + 2, r.y));
    r
}

fn seed(app: &mut App, slugs: &[&str]) {
    let req = postui_core::model::HttpRequest {
        method: postui_core::model::Method::Get,
        url: "https://example.test/x".into(),
        substitute_body: false,
        params: Default::default(),
        headers: Default::default(),
        variables: Default::default(),
        body: None,
    };
    for slug in slugs {
        postui_core::storage::save_request(&app.project.root, slug, &req).unwrap();
    }
    app.update(Action::RefreshSidebar);
}

/// Opens every folder in the tree, so `Row::Request` rows for nested slugs
/// exist (folders start collapsed).
fn expand_all(app: &mut App) {
    while let Some(i) = app.sidebar.rows.iter().position(|r| {
        matches!(
            r,
            Row::Folder {
                expanded: false,
                ..
            }
        )
    }) {
        app.sidebar.selected = Some(i);
        app.update(Action::ToggleSelectedFolder);
    }
    app.sidebar.selected = None;
}

/// Writes a request file that cannot parse, so the sidebar lists it as a
/// broken row.
fn seed_broken(app: &mut App, slug: &str) {
    let path = app
        .project
        .root
        .join("requests")
        .join(format!("{slug}.toml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "this is = = not toml\n").unwrap();
    app.update(Action::RefreshSidebar);
}

fn menu_labels(app: &App) -> Vec<String> {
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        panic!("expected a Dropdown (context menu) on top");
    };
    state.items.iter().map(|i| i.label.clone()).collect()
}

fn row_index_of(app: &App, slug: &str) -> usize {
    app.sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Request { slug: s, .. } if s == slug))
        .unwrap_or_else(|| panic!("no sidebar row for {slug}"))
}

// --- the headline flow ---------------------------------------------------

#[test]
fn right_click_sidebar_row_opens_menu_and_duplicate_creates_copy() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    let row = row_index_of(&app, "users/list");

    press(&mut app, Hit::SidebarRow(row), right_down);
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));
    assert_eq!(
        menu_labels(&app),
        vec!["Open", "Duplicate", "Rename…", "Delete…"],
    );

    press(&mut app, Hit::DropdownRow(1), left_down);
    assert!(postui_core::storage::request_exists(
        &app.project.root,
        "users/list-copy"
    ));
    assert_eq!(app.editor.slug.as_deref(), Some("users/list-copy"));
    assert!(app.modals.top().is_none(), "the menu closed on activation");
}

// --- menu mechanics ------------------------------------------------------

#[test]
fn right_click_on_empty_space_opens_no_menu() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    render(&mut app);
    let r = app.hits.rect_of(&Hit::Pane(PaneId::Response)).unwrap();
    app.handle_mouse(right_down(r.x + 2, r.y + r.height - 2));
    assert!(
        app.modals.top().is_none(),
        "no context menu for the response pane background"
    );
}

#[test]
fn disabled_menu_item_click_runs_nothing_and_leaves_the_menu_open() {
    let mut app = App::new_for_test();
    seed_broken(&mut app, "oops");
    let row = row_index_of(&app, "oops");

    press(&mut app, Hit::SidebarRow(row), right_down);
    let labels = menu_labels(&app);
    assert_eq!(labels.first().map(String::as_str), Some("Open"));
    let Some(Modal::Dropdown(state)) = app.modals.top() else {
        unreachable!()
    };
    assert!(
        state.items[0].action.is_none(),
        "a broken request cannot be opened, so Open is disabled"
    );
    assert_ne!(
        state.selected, 0,
        "the keyboard cursor opens on an enabled row, not the disabled one"
    );

    press(&mut app, Hit::DropdownRow(0), left_down);
    assert!(
        matches!(app.modals.top(), Some(Modal::Dropdown(_))),
        "clicking a disabled item leaves the menu open"
    );
    assert_eq!(app.editor.slug, None, "and runs nothing");
}

#[test]
fn broken_row_menu_offers_show_error() {
    let mut app = App::new_for_test();
    seed_broken(&mut app, "oops");
    let row = row_index_of(&app, "oops");
    press(&mut app, Hit::SidebarRow(row), right_down);
    let labels = menu_labels(&app);
    let i = labels.iter().position(|l| l == "Show error…").unwrap();
    press(&mut app, Hit::DropdownRow(i), left_down);
    assert!(matches!(app.modals.top(), Some(Modal::Message { .. })));
}

#[test]
fn click_away_closes_the_menu_without_activating_what_is_under_it() {
    let mut app = App::new_for_test();
    let slugs: Vec<String> = (0..12).map(|i| format!("r{i:02}")).collect();
    let refs: Vec<&str> = slugs.iter().map(String::as_str).collect();
    seed(&mut app, &refs);

    press(&mut app, Hit::SidebarRow(0), right_down);
    assert!(matches!(app.modals.top(), Some(Modal::Dropdown(_))));

    // Pick the first sidebar row that is clear of the open menu's panel, so
    // the click is unambiguously "outside" it.
    render(&mut app);
    let menu = app.hits.rect_of(&Hit::ModalBody).unwrap();
    let under = (1..12)
        .find(|i| {
            app.hits
                .rect_of(&Hit::SidebarRow(*i))
                .is_some_and(|r| r.y >= menu.y + menu.height)
        })
        .expect("a sidebar row below the menu");
    press(&mut app, Hit::SidebarRow(under), left_down);
    assert!(app.modals.top().is_none(), "click-away closed the menu");
    assert_eq!(
        app.editor.slug, None,
        "and was swallowed — the row under it did not open"
    );
}

#[test]
fn esc_closes_the_context_menu() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    let row = row_index_of(&app, "users/list");
    press(&mut app, Hit::SidebarRow(row), right_down);
    let keymap = Keymap::default_bindings();
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.top().is_none());
}

#[test]
fn context_menu_is_keyboard_navigable() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    let row = row_index_of(&app, "users/list");
    press(&mut app, Hit::SidebarRow(row), right_down);

    let keymap = Keymap::default_bindings();
    // Open, Duplicate — one Down lands on Duplicate.
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(&keymap, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(postui_core::storage::request_exists(
        &app.project.root,
        "users/list-copy"
    ));
}

// --- folder rows ---------------------------------------------------------

#[test]
fn right_click_folder_row_offers_new_request_here_and_expand() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    // Folders start collapsed.
    let folder = app
        .sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Folder { .. }))
        .expect("a folder row for users/");

    press(&mut app, Hit::SidebarRow(folder), right_down);
    assert_eq!(menu_labels(&app), vec!["New request here…", "Expand"]);

    press(&mut app, Hit::DropdownRow(0), left_down);
    let Some(Modal::Prompt { kind, input, .. }) = app.modals.top() else {
        panic!("expected the new-request prompt");
    };
    assert_eq!(*kind, PromptKind::NewRequest);
    assert_eq!(input.text(), "users/", "prefilled with the folder path");
}

#[test]
fn expanded_folder_row_offers_collapse() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    let folder = app
        .sidebar
        .rows
        .iter()
        .position(|r| matches!(r, Row::Folder { .. }))
        .unwrap();

    press(&mut app, Hit::SidebarRow(folder), right_down);
    assert_eq!(menu_labels(&app), vec!["New request here…", "Collapse"]);
    press(&mut app, Hit::DropdownRow(1), left_down);
    assert!(
        app.sidebar
            .rows
            .iter()
            .all(|r| !matches!(r, Row::Request { .. })),
        "the folder collapsed, hiding its request"
    );
}

// --- duplicate from the keyboard / palette -------------------------------

#[test]
fn duplicate_request_is_bound_and_in_the_palette() {
    let keymap = Keymap::default_bindings();
    assert_eq!(
        keymap.lookup(&KeyCombo::parse("ctrl+shift+d").unwrap()),
        Some(Action::DuplicateRequest),
    );
    assert!(
        postui::components::palette::all_commands()
            .iter()
            .any(|c| c.id == "request-duplicate" && c.action == Action::DuplicateRequest),
    );
}

#[test]
fn duplicate_request_action_acts_on_the_selected_row() {
    let mut app = App::new_for_test();
    seed(&mut app, &["users/list"]);
    expand_all(&mut app);
    app.sidebar.selected = Some(row_index_of(&app, "users/list"));
    app.update(Action::DuplicateRequest);
    assert!(postui_core::storage::request_exists(
        &app.project.root,
        "users/list-copy"
    ));
    assert_eq!(app.editor.slug.as_deref(), Some("users/list-copy"));

    // A second duplicate of the original does not collide.
    app.sidebar.selected = Some(row_index_of(&app, "users/list"));
    app.update(Action::DuplicateRequest);
    assert!(postui_core::storage::request_exists(
        &app.project.root,
        "users/list-copy-2"
    ));
}
