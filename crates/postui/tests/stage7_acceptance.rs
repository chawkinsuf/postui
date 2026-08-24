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
use tokio::sync::mpsc::UnboundedReceiver;

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
        name: None,
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

// --- shared helpers for the goal-by-goal scenarios below -----------------

/// Re-renders (so `app.hits` is fresh) and clicks the centre of `hit` —
/// the plain mouse-only "click this control" the scenarios below are made
/// of. `press` above aims at a row's left edge instead, which is what the
/// menu tests want; here the centre is what a user hits.
fn click(app: &mut App, hit: Hit) {
    render(app);
    let r = app
        .hits
        .rect_of(&hit)
        .unwrap_or_else(|| panic!("no rect registered for {hit:?}"));
    app.handle_mouse(left_down(r.x + r.width / 2, r.y + r.height / 2));
}

/// A project with the given files written *before* it is opened, so
/// declarations that only load at open time (project.toml, variables.toml)
/// are in force. The `TempDir` and the receiver must outlive the `App`.
type TestApp = (App, tempfile::TempDir, UnboundedReceiver<Action>);

fn app_in_project(files: &[(&str, &str)]) -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    for (name, body) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (App::with_root(tx, dir.path().to_path_buf()), dir, rx)
}

/// Clicks a request's sidebar row, which is how a mouse-only user opens it.
fn open_request(app: &mut App, slug: &str) {
    let row = row_index_of(app, slug);
    press(app, Hit::SidebarRow(row), left_down);
}

fn type_text(app: &mut App, keymap: &Keymap, text: &str) {
    for c in text.chars() {
        app.handle_key(keymap, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn key(app: &mut App, keymap: &Keymap, code: KeyCode) {
    app.handle_key(keymap, KeyEvent::new(code, KeyModifiers::NONE));
}

// --- goal 2: saving is mouse-reachable -----------------------------------

#[test]
fn a_request_is_opened_edited_and_saved_with_nothing_but_clicks() {
    let mut app = App::new_for_test();
    seed(&mut app, &["ping"]);
    let row = row_index_of(&app, "ping");

    // Open it: one click on its sidebar row.
    press(&mut app, Hit::SidebarRow(row), left_down);
    assert_eq!(app.editor.slug.as_deref(), Some("ping"));
    assert!(!app.editor.is_dirty());

    // Dirty it without touching the keyboard: the method chip's dropdown.
    click(&mut app, Hit::MethodSelector);
    let post = postui_core::model::Method::ALL
        .iter()
        .position(|m| *m == postui_core::model::Method::Post)
        .unwrap();
    click(&mut app, Hit::DropdownRow(post));
    assert_eq!(app.editor.method, postui_core::model::Method::Post);
    assert!(app.editor.is_dirty(), "the change is unsaved");

    // The toolbar's Save chip is on screen...
    let frame = render(&mut app);
    assert!(
        frame.contains("save \u{2022}"),
        "the toolbar's save chip carries the dirty dot"
    );

    // ...and clicking it writes the file.
    click(&mut app, Hit::FooterChip(Action::SaveRequest));
    assert!(!app.editor.is_dirty(), "the click saved");
    let on_disk = postui_core::storage::load_request(&app.project.root, "ping").unwrap();
    assert_eq!(on_disk.method, postui_core::model::Method::Post);
}

// --- goal 3: body clicks land where they were aimed ----------------------

#[test]
fn clicking_a_body_line_puts_the_caret_at_that_lines_end() {
    let mut app = App::new_for_test();
    seed(&mut app, &["ping"]);
    open_request(&mut app, "ping");

    // Draw position 3 is the Body tab (Params, Headers, Vars, Body).
    click(&mut app, Hit::EditorTab(3));
    app.editor.set_body_text("{\n  \"aa\": 1,\n}\n");

    render(&mut app);
    let body = app.hits.rect_of(&Hit::BodyEditor).expect("the body editor");
    // Far past the end of the second line (`  "aa": 1,`, 10 characters).
    app.handle_mouse(left_down(body.right() - 2, body.y + 1));
    assert_eq!(
        (app.editor.body.cursor.row, app.editor.body.cursor.col),
        (1, 10),
        "the caret lands at the end of the clicked line, not the document"
    );

    // Below the last line: the end of the last line.
    app.handle_mouse(left_down(body.x + 4, body.bottom() - 1));
    assert_eq!(
        (app.editor.body.cursor.row, app.editor.body.cursor.col),
        (3, 0),
        "a click in the void lands at the end of the last line"
    );
}

// --- goal 4: the headers actually sent are visible -----------------------

#[test]
fn the_headers_tab_shows_defaults_auto_content_type_and_host_resolved() {
    // Written before the project opens: the computed section must show what
    // the project's own config contributes, not just the request's rows.
    let (mut app, _dir, _rx) = app_in_project(&[
        (
            "project.toml",
            "name = \"svc\"\n[default_headers]\nx-team = \"{{team}}\"\n",
        ),
        ("variables.toml", "[team]\ndefault = \"payments\"\n"),
    ]);

    seed(&mut app, &["ping"]);
    open_request(&mut app, "ping");
    app.editor.url = postui::components::line_input::LineInput::new("https://api.example.test/v1");
    app.editor.set_body_text("{\"a\": 1}");
    click(&mut app, Hit::EditorTab(1));

    let frame = render(&mut app);
    for want in ["x-team", "payments", "Content-Type", "application/json"] {
        assert!(
            frame.contains(want),
            "the Headers tab shows {want}: {frame}"
        );
    }
    assert!(
        frame.contains("Host: api.example.test"),
        "and the client-generated Host with the real host: {frame}"
    );
}

// --- goal 5: a variable's value is visible from inside the request -------

#[test]
fn hovering_a_url_token_pops_its_value_and_scope() {
    let mut app = App::new_for_test();
    app.project
        .edit_variables(|_| Ok("[base_url]\ndefault = \"http://fallback\"\n".to_string()))
        .unwrap();
    app.project
        .edit_env("qa", |_| Ok("base_url = \"http://qa.test\"\n".to_string()))
        .unwrap();
    app.project.set_env(Some("qa".into()));
    app.editor.url = postui::components::line_input::LineInput::new("{{base_url}}/x");
    app.update(Action::Render);

    render(&mut app);
    let token = app
        .hits
        .rect_of(&Hit::VarToken("base_url".into()))
        .expect("the URL's token is a hit target");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: token.x + 1,
        row: token.y,
        modifiers: KeyModifiers::NONE,
    });

    let frame = render(&mut app);
    assert!(
        frame.contains("base_url = http://qa.test"),
        "the tooltip names the value: {frame}"
    );
    assert!(frame.contains("env qa"), "and its scope: {frame}");
}

// --- goal 6: in-place table editing --------------------------------------

#[test]
fn a_param_cell_commits_on_click_away_reverts_on_esc_and_the_ghost_row_creates() {
    let mut app = App::new_for_test();
    seed(&mut app, &["ping"]);
    open_request(&mut app, "ping");
    let keymap = Keymap::default_bindings();
    assert!(app.editor.params.is_empty());

    // The ghost row is row 0 of an empty table: click into its key cell and
    // type — no select-then-edit dance.
    click(&mut app, Hit::TableCell { row: 0, col: 0 });
    type_text(&mut app, &keymap, "page");
    click(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_text(&mut app, &keymap, "2");

    // Clicking outside the table commits rather than discarding.
    click(&mut app, Hit::Pane(PaneId::Response));
    assert_eq!(
        app.editor.params.get("page").map(|e| e.value.as_str()),
        Some("2"),
        "the ghost row became a real param"
    );

    // Esc reverts the active cell to its pre-edit value.
    click(&mut app, Hit::TableCell { row: 0, col: 1 });
    type_text(&mut app, &keymap, "99");
    key(&mut app, &keymap, KeyCode::Esc);
    assert_eq!(
        app.editor.params.get("page").map(|e| e.value.as_str()),
        Some("2"),
        "Esc put the old value back"
    );
}

// --- goal 7: the whole variables story -----------------------------------

/// A project written in the stage-6 variable format — the shape the user's
/// real project had before this stage.
fn legacy_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "variables.toml",
            "[base_url]\ndefault = \"http://localhost:8080\"\n\n\
             [tier]\ndescription = \"pricing tier\"\n[tier.options.gold]\nvalue = \"g-1\"\n\n\
             [groups.user]\nmembers = [\"user_id\", \"customer_id\"]\n\
             [groups.user.options.alice]\nuser_id = \"1001\"\ncustomer_id = \"c-77\"\n",
        ),
        (
            "environments/qa.toml",
            "[options.tier.gold]\nvalue = \"g-qa\"\n",
        ),
    ]
}

#[test]
fn a_legacy_project_migrates_then_grows_a_group_whose_selection_drives_resolution() {
    let (mut app, _dir, _rx) = app_in_project(&legacy_files());

    // --- the migration confirm, answered with the mouse ---
    let Some(Modal::Confirm { title, .. }) = app.modals.top() else {
        panic!("a legacy project offers the migration on open");
    };
    assert_eq!(title, "Migrate variables");
    click(&mut app, Hit::ConfirmChoice('y'));
    assert!(app.modals.is_empty(), "answering closes the prompt");
    assert!(app.project.pending_migration().is_none());
    assert_eq!(
        app.project.model.groups["user"].fields,
        ["user_id", "customer_id"],
        "`members` became `fields`"
    );
    assert!(
        app.project.model.groups.contains_key("tier"),
        "the enumerated variable became a one-field group"
    );

    // --- into the Manager, from the header chip ---
    click(&mut app, Hit::HeaderVars);
    assert_eq!(app.screen, postui::app::Screen::VarManager);
    // Entries belong to an environment, so pick one — from the Manager's
    // own `Environment: … \u{25be}` switcher. `qa` is the only one.
    assert_eq!(app.project.active_env, None);
    click(&mut app, Hit::VmEnvSwitch);
    click(&mut app, Hit::ChooserRow(0));
    assert_eq!(app.project.active_env.as_deref(), Some("qa"));
    let keymap = Keymap::default_bindings();

    // --- create a group: the [+ Group] button, then its prompt ---
    click(&mut app, Hit::VmNewGroup);
    type_text(&mut app, &keymap, "region");
    key(&mut app, &keymap, KeyCode::Tab);
    type_text(&mut app, &keymap, "zone,dc");
    key(&mut app, &keymap, KeyCode::Enter);
    assert_eq!(
        app.project.model.groups["region"].fields,
        ["zone", "dc"],
        "the group is declared with its fields"
    );

    // --- open it in the detail pane ---
    let row = left_row_of(&app, "region");
    click(&mut app, Hit::VmLeftRow(row));

    // --- two entries, typed into the ghost row ---
    add_entry(&mut app, &keymap, &["eu", "eu-west-1", "dub"]);
    add_entry(&mut app, &keymap, &["us", "us-east-1", "iad"]);
    let entries = postui_core::varmodel::group_entries(&app.project.env_data, "region")
        .expect("the group has entries in qa");
    assert_eq!(entries.keys().collect::<Vec<_>>(), ["eu", "us"]);
    assert_eq!(entries["us"].values["dc"], "iad");

    // Until one is picked, the group's fields do not resolve at all.
    assert!(
        !app.project.resolved.values.contains_key("zone"),
        "no selection means no value — that is the point of the model"
    );

    // --- flip the selection with the radio column ---
    click(&mut app, Hit::VmEntryRadio(0));
    assert_eq!(app.project.resolved.values["zone"], "eu-west-1");
    assert_eq!(app.project.resolved.values["dc"], "dub");
    click(&mut app, Hit::VmEntryRadio(1));
    assert_eq!(
        app.project.resolved.values["zone"], "us-east-1",
        "flipping the radio re-resolves every field of the group at once"
    );
    assert_eq!(app.project.resolved.values["dc"], "iad");

    // ...and the request sees it: a `{{zone}}` token in the URL resolves.
    click(&mut app, Hit::FooterChip(Action::CloseScreen));
    app.editor.url = postui::components::line_input::LineInput::new("http://{{zone}}/x");
    app.update(Action::Render);
    render(&mut app);
    let token = app.hits.rect_of(&Hit::VarToken("zone".into())).unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: token.x + 1,
        row: token.y,
        modifiers: KeyModifiers::NONE,
    });
    let frame = render(&mut app);
    assert!(
        frame.contains("zone = us-east-1"),
        "the request shows the selected entry's value: {frame}"
    );
    assert!(
        frame.contains("group region \u{2192} \"us\""),
        "...and names the group and entry it came from: {frame}"
    );
}

/// Index of the Manager left-list row declaring `name`.
fn left_row_of(app: &App, name: &str) -> usize {
    app.varmanager
        .left_rows
        .iter()
        .position(|r| r.name() == Some(name))
        .unwrap_or_else(|| panic!("no left-list row for {name}"))
}

/// Types one whole entry into the group grid's ghost row: `cells[0]` is the
/// entry name, the rest are its field values, `Tab` between them.
fn add_entry(app: &mut App, keymap: &Keymap, cells: &[&str]) {
    click(app, Hit::VmNewEntry);
    for (i, cell) in cells.iter().enumerate() {
        type_text(app, keymap, cell);
        if i + 1 < cells.len() {
            key(app, keymap, KeyCode::Tab);
        }
    }
    key(app, keymap, KeyCode::Enter);
}

// --- goal 8: no pretty-print cap -----------------------------------------

/// Three megabytes of JSON: Raw is readable the moment the response lands,
/// the tree arrives later over the action channel, and nothing anywhere
/// says the body was too big to format.
#[tokio::test]
async fn a_three_megabyte_json_body_shows_raw_at_once_and_pretty_when_it_parses() {
    use postui::components::response::ViewMode;

    let big = format!("{{\"blob\": \"{}\"}}", "x".repeat(3 * 1024 * 1024));
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/big"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(big.clone()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    postui_core::project::init_project(dir.path(), Some("svc")).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, dir.path().to_path_buf());
    app.editor.url =
        postui::components::line_input::LineInput::new(&format!("{}/big", server.uri()));

    app.update(Action::ForceSend);
    let generation = app.session.send_generation;
    loop {
        let action = recv(&mut rx).await;
        let settled = matches!(
            &action,
            Action::ResponseArrived { generation: g, .. } | Action::RequestFailed { generation: g, .. }
                if *g == generation
        );
        app.update(action);
        if settled {
            break;
        }
    }

    // Raw, immediately — no cap, no forced-Raw-forever gate.
    let view = app.session.response.view().expect("a response");
    assert_eq!(view.mode, ViewMode::Raw);
    assert!(
        view.parsing && view.tree.is_none(),
        "the parse is off-thread"
    );
    let frame = render(&mut app);
    assert!(
        !frame.to_lowercase().contains("too large"),
        "nothing tells the user the body was too big: {frame}"
    );

    // ...then the parse reports in over the same channel a send does.
    loop {
        let action = recv(&mut rx).await;
        let parsed = matches!(action, Action::PrettyParsed { .. });
        app.update(action);
        if parsed {
            break;
        }
    }
    let view = app.session.response.view().unwrap();
    assert!(!view.parsing && view.tree.is_some(), "the tree is ready");

    // And the Pretty tab now shows it — reached by clicking the tab.
    click(&mut app, Hit::ResponseTab(ViewMode::Pretty));
    let view = app.session.response.view().unwrap();
    assert_eq!(view.mode, ViewMode::Pretty);
    assert!(view.visible_len() > 1, "with the parsed lines under it");
}

async fn recv(rx: &mut UnboundedReceiver<Action>) -> Action {
    tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv())
        .await
        .expect("timed out waiting for a background result")
        .expect("the action channel closed early")
}
