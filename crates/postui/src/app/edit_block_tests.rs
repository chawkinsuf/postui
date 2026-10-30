//! The edit flag on the app's side (spec 2026-10-28 §4, piece 3): the lock
//! an open project holds, a second window kept out, and the waiting screen
//! an open of a flagged project shows. The test plays the agent by raising
//! and lowering the flag through `Project` directly, as `postui edit` does.

use super::*;
use postui_core::project::{EditFlag, ProjectLock};
use ratatui::crossterm::event::KeyCode;
use std::path::Path;

fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn root(app: &App) -> PathBuf {
    app.root().to_path_buf()
}

/// The agent's `edit begin`, without its check of the lock.
fn raise(root: &Path, note: &str) {
    Project::raise_edit_flag(root, &EditFlag::new(Some(note.into())), false).unwrap();
}

/// The agent's `edit end`.
fn lower(root: &Path) {
    assert!(Project::lower_edit_flag(root).unwrap());
}

/// A tick that polls, whenever the last one was.
fn poll(app: &mut App) {
    app.edit_flag_polled = None;
    app.update(Action::Tick);
}

/// A test app on a folder with a `project.toml`: one that holds its lock.
fn project_app() -> App {
    let mut app = App::new_for_test();
    postui_core::fixtures::init_project(&root(&app), None).unwrap();
    app.take_project_lock();
    assert!(app.project_lock.is_some());
    app
}

/// A project of its own, kept alive by the returned directory.
fn other_project(name: &str) -> tempfile::TempDir {
    let other = tempfile::tempdir().unwrap();
    postui_core::fixtures::init_project(other.path(), Some(name)).unwrap();
    other
}

/// An app waiting behind the screen on `flagged`, an agent's edit of it
/// under way.
fn waiting_on(flagged: &Path) -> App {
    let mut app = project_app();
    if !Project::edit_flag_up(flagged) {
        raise(flagged, "add orders");
    }
    app.update(Action::ForceSwitchProject(flagged.to_path_buf()));
    assert!(app.edit_block.as_ref().is_some_and(|b| b.root == flagged));
    app
}

fn screen_text(app: &mut App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>() + "\n")
        .collect()
}

/// The open project's lock: the agent's `edit begin` sees it, and it goes
/// with the app.
#[test]
fn an_open_project_holds_its_lock_until_the_app_goes() {
    let app = project_app();
    let root = root(&app);
    assert!(Project::tui_open(&root).unwrap());
    drop(app);
    assert!(!Project::tui_open(&root).unwrap());
}

/// A second window doesn't open a project the first has open (the user:
/// "prevent two instances of the tui from opening the same project"): at
/// launch it runs empty, saying so, and a switch to it stays put.
#[test]
fn a_second_window_doesnt_open_the_project() {
    let first = project_app();
    let taken = root(&first);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let second = App::with_root(tx, taken.clone());
    assert!(second.project.is_none());
    assert!(second.open_error.as_ref().is_some_and(|e| e.to_string().contains("another postui")));
    assert!(second.sidebar.notice.as_ref().is_some_and(|n| n.contains("Reload from disk")));

    let mut third = project_app();
    let here = root(&third);
    third.update(Action::ForceSwitchProject(taken.clone()));
    assert_eq!(root(&third), here, "the switch refused");
    assert!(third.toasts.messages().iter().any(|m| m.contains("open in another postui window")));
    drop(first);
}

/// A launch onto a flagged project waits behind the screen and opens
/// nothing (`Project::open` would empty the trash) until the flag comes
/// down; it holds no lock meanwhile, which the agent's edit needs.
#[test]
fn launching_onto_a_flagged_project_waits_to_open_it() {
    let dir = other_project("Kit");
    let root = dir.path().to_path_buf();
    let trashed = root.join(postui_core::project::TRASH_DIR).join("1/old.toml");
    std::fs::create_dir_all(trashed.parent().unwrap()).unwrap();
    std::fs::write(&trashed, "x").unwrap();
    raise(&root, "x");

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::with_root(tx, root.clone());
    assert!(app.project.is_none());
    assert!(app.edit_block.as_ref().is_some_and(|b| b.root == root));
    assert!(trashed.exists(), "nothing opened the project");
    assert!(!Project::tui_open(&root).unwrap());
    app.handle_key(plain('x'));
    poll(&mut app);
    assert!(app.edit_block.is_some());

    lower(&root);
    poll(&mut app);
    assert!(app.edit_block.is_none());
    assert_eq!(app.project.as_ref().map(|p| p.root().to_path_buf()), Some(root.clone()));
    assert!(!trashed.exists(), "opened now");
    assert!(Project::tui_open(&root).unwrap(), "and locked");
}

/// A switch onto a flagged project leaves the current one as any switch
/// does, a Discard of its edits included, and lets go of its lock. The
/// project switched to joins the known projects only once it opens.
#[test]
fn a_switch_onto_a_flagged_project_leaves_the_current_one() {
    let mut app = project_app();
    let here = root(&app);
    let req = postui_core::model::HttpRequest::from_toml_str(r#"url = "https://x/r""#).unwrap();
    postui_core::fixtures::save_request(&here, "main/r", &req).unwrap();
    app.update(Action::RefreshSidebar);
    app.update(Action::ForceOpenRequest("main/r".into()));
    app.focus = PaneId::Editor;
    app.editor.open_url_from_app();
    app.handle_key(plain('/'));
    assert!(app.editor.is_dirty());
    let other = other_project("Other");
    raise(other.path(), "x");

    app.update(Action::SwitchProject(other.path().to_path_buf()));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })), "the unsaved question");
    app.handle_key(plain('d'));
    assert!(app.edit_block.as_ref().is_some_and(|b| b.root == other.path()));
    assert!(app.project.is_none());
    assert!(!app.editor.is_dirty(), "discarded, as asked");
    assert!(!Project::tui_open(&here).unwrap());
    assert!(!app.registry.known.iter().any(|p| p == other.path()));
    lower(other.path());
    poll(&mut app);
    assert_eq!(app.registry.last.as_deref(), Some(other.path()));
}

/// Switch projects from the screen opens whichever is picked, the one
/// just left included, and the screen goes.
#[test]
fn switching_from_the_screen_opens_the_project_left() {
    let mut app = project_app();
    let here = root(&app);
    let other = other_project("Other");
    raise(other.path(), "x");
    app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
    assert!(app.edit_block.is_some());
    app.update(Action::SwitchProject(here.clone()));
    assert!(app.edit_block.is_none());
    assert_eq!(root(&app), here);
    assert!(Project::tui_open(&here).unwrap());
}

#[test]
fn the_screens_keys_switch_and_quit() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    app.handle_key(plain('s'));
    assert!(
        matches!(app.modals.top(), Some(Modal::Chooser(c)) if c.title == "Projects"),
        "s opens the projects chooser over the screen"
    );
    // The chooser takes keys as anywhere: Esc closes it.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.modals.is_empty());
    assert!(app.edit_block.is_some());

    // ctrl+c quits on its second press, as everywhere.
    app.handle_key(ctrl('c'));
    assert!(!app.should_quit);
    app.handle_key(ctrl('c'));
    assert!(app.should_quit);

    let mut app = waiting_on(other.path());
    app.handle_key(plain('q'));
    assert!(app.should_quit);
}

/// Override asks, then takes the flag down and opens the project.
#[test]
fn override_asks_then_removes_the_flag_and_opens() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    app.handle_key(plain('o'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    assert!(Project::edit_flag_up(other.path()), "nothing happens before the answer");
    app.handle_key(plain('o'));
    assert!(!Project::edit_flag_up(other.path()));
    assert!(app.edit_block.is_none() && app.modals.is_empty());
    assert_eq!(root(&app), other.path());
}

/// Override takes down only the edit it asked about: one another `edit
/// begin` raised since, even one the poll shows, is left up.
#[test]
fn override_takes_down_only_the_edit_it_asked_about() {
    let other = other_project("Other");
    let root = other.path().to_path_buf();
    let mut app = waiting_on(&root);
    app.handle_key(plain('o'));
    let second = EditFlag::new(Some("second".into()));
    Project::raise_edit_flag(&root, &second, true).unwrap();
    poll(&mut app);
    app.handle_key(plain('o'));
    assert_eq!(Project::read_edit_flag(&root).unwrap(), Some(second.clone()));
    assert_eq!(app.edit_block.as_ref().and_then(|b| b.flag.clone()), Some(second));

    // Nor one gone unreadable since.
    app.handle_key(plain('o'));
    std::fs::write(root.join(".postui/editing.toml"), "started = [").unwrap();
    app.handle_key(plain('o'));
    assert!(Project::edit_flag_up(&root), "not lowered");
    assert!(app.edit_block.as_ref().is_some_and(|b| b.flag.is_none()));
}

/// Override waits its turn while an agent's command is changing the flag
/// (`Project::hold_edit_flag`): nothing is taken down meanwhile.
#[test]
fn override_waits_while_a_command_changes_the_flag() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    let hold = Project::hold_edit_flag(other.path()).unwrap().expect("free");
    app.handle_key(plain('o'));
    app.handle_key(plain('o'));
    assert!(Project::edit_flag_up(other.path()));
    assert!(app.toasts.messages().iter().any(|m| m.contains("try again")));
    drop(hold);
    app.handle_key(plain('o'));
    app.handle_key(plain('o'));
    assert!(!Project::edit_flag_up(other.path()));
}

/// alt+z cycles on past a project an agent is editing.
#[test]
fn the_project_cycle_goes_on_from_the_screen() {
    let mut app = project_app();
    let here = root(&app);
    let other = other_project("Other");
    raise(other.path(), "x");
    // The flagged project first: the cycle goes on from it, not from the
    // first known.
    app.registry.known = vec![other.path().to_path_buf(), here.clone()];
    app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
    assert!(app.edit_block.is_some());
    let next = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT);
    assert!(matches!(app.lookup_key(&KeyCombo::from_event(&next)), Some(Action::CycleProject(_))));
    app.handle_key(next);
    assert!(app.edit_block.is_none());
    assert_eq!(root(&app), here);
}

/// A project made in place that another window also has open opens all
/// the same, saying it isn't locked.
#[test]
fn a_project_made_in_place_says_when_it_cant_lock() {
    let mut app = project_app();
    let other = ProjectLock::take(&root(&app));
    assert!(other.unwrap().is_none(), "this app holds it");
    app.project_lock = None;
    let held = ProjectLock::take(&root(&app)).unwrap().expect("free now");
    app.take_project_lock();
    assert!(app.project_lock.is_none());
    assert!(app.toasts.messages().iter().any(|m| m.contains("another postui window")));
    drop(held);
}

/// The Override question closes when the edit ends on its own.
#[test]
fn the_override_question_goes_with_the_edit() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    app.handle_key(plain('o'));
    assert!(matches!(app.modals.top(), Some(Modal::Confirm { .. })));
    lower(other.path());
    poll(&mut app);
    assert!(app.edit_block.is_none());
    assert!(app.modals.is_empty());
}

/// The user's own dialog over the screen (the projects chooser) isn't
/// opened under: the project opens once it closes.
#[test]
fn the_open_waits_for_the_users_dialog() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    app.handle_key(plain('s'));
    assert!(matches!(app.modals.top(), Some(Modal::Chooser(_))));
    lower(other.path());
    poll(&mut app);
    assert!(app.edit_block.is_some() && app.project.is_none(), "still waiting");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    poll(&mut app);
    assert!(app.edit_block.is_none());
    assert_eq!(root(&app), other.path());
}

/// A waiting project that won't open once its flag comes down is the
/// app's state as a refused launch is: shown, and retried by a reload.
#[test]
fn a_waiting_project_that_wont_open_says_so() {
    let other = other_project("Other");
    let root = other.path().to_path_buf();
    let mut app = waiting_on(&root);
    std::fs::write(root.join("project.toml"), "name = [").unwrap();
    lower(&root);
    poll(&mut app);
    assert!(app.edit_block.is_none() && app.project.is_none());
    assert!(app.open_error.is_some());
    assert!(app.sidebar.notice.is_some());
    assert!(!Project::tui_open(&root).unwrap(), "no project, no lock");
}

/// Focus coming back looks at the flag at once.
#[test]
fn focus_gained_looks_at_the_flag_now() {
    let other = other_project("Other");
    let mut app = waiting_on(other.path());
    app.edit_flag_polled = Some(Instant::now());
    lower(other.path());
    app.on_focus_gained();
    assert!(app.edit_block.is_none());
    assert_eq!(root(&app), other.path());
}

/// The screen covers the frame: its buttons are the only hits, and a
/// click on one runs it. The note's control chars show as their codes.
#[test]
fn the_screen_draws_its_buttons_and_they_click() {
    let other = other_project("Other");
    let mut app = project_app();
    raise(other.path(), "add the orders\nrequests\x1b");
    app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
    let screen = screen_text(&mut app, 100, 30);
    assert!(screen.contains("Updates in progress"), "{screen}");
    assert!(screen.contains("\"add the orders^Jrequests^[\""), "{screen}");
    assert!(screen.contains("Switch projects"), "{screen}");
    assert!(!screen.contains("If the agent has stopped"), "a fresh flag: {screen}");

    let quit = (0..30)
        .flat_map(|y| (0..100).map(move |x| (x, y)))
        .find(|&(x, y)| app.hits.hit_at(x, y) == Some(&Hit::EditBlock(EditBlockButton::Quit)))
        .expect("the Quit button is drawn");
    let press = ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(
            ratatui::crossterm::event::MouseButton::Left,
        ),
        column: quit.0,
        row: quit.1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(press);
    assert!(app.should_quit);
}

/// An old flag says the agent may have stopped, and points at Override.
#[test]
fn an_old_flag_points_at_override() {
    let other = other_project("Other");
    let old = EditFlag { started: "2026-01-01T00:00:00Z".into(), note: None, id: "old".into() };
    Project::raise_edit_flag(other.path(), &old, false).unwrap();
    let mut app = project_app();
    app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
    let screen = screen_text(&mut app, 100, 30);
    assert!(screen.contains("If the agent has stopped, Override opens it now."), "{screen}");
}

/// The card keeps its size and its buttons their place as the flag ages,
/// the old flag's hint included.
#[test]
fn the_card_doesnt_move_as_the_flag_ages() {
    let quit_at = |started: &str| {
        let other = other_project("Other");
        let flag = EditFlag { started: started.into(), note: Some("n".into()), id: "a".into() };
        Project::raise_edit_flag(other.path(), &flag, false).unwrap();
        let mut app = project_app();
        app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
        screen_text(&mut app, 100, 30);
        (0..30)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .find(|&(x, y)| app.hits.hit_at(x, y) == Some(&Hit::EditBlock(EditBlockButton::Quit)))
    };
    let now = postui_core::project::rfc3339(std::time::SystemTime::now());
    let ten_minutes =
        postui_core::project::rfc3339(std::time::SystemTime::now() - Duration::from_secs(10 * 60));
    let young = quit_at(&now);
    assert!(young.is_some());
    assert_eq!(quit_at(&ten_minutes), young, "the age grew longer");
    assert_eq!(quit_at("2026-01-01T00:00:00Z"), young, "the old flag's hint came");
}

/// The open project by another spelling of its path is no switch: it
/// would meet its own lock.
#[cfg(unix)]
#[test]
fn the_open_project_by_another_path_is_no_switch() {
    let mut app = project_app();
    let here = root(&app);
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("here");
    std::os::unix::fs::symlink(&here, &link).unwrap();
    assert!(!app.update(Action::SwitchProject(link)));
    assert_eq!(root(&app), here);
    assert!(!app.toasts.messages().iter().any(|m| m.contains("another postui window")));
}

/// On a short terminal the card drops body lines, never its buttons, and
/// a note of any length draws.
#[test]
fn a_short_terminal_keeps_the_buttons_and_a_huge_note_draws() {
    let other = other_project("Other");
    let mut app = project_app();
    raise(other.path(), &"x".repeat(70_000));
    app.update(Action::ForceSwitchProject(other.path().to_path_buf()));
    screen_text(&mut app, 80, 7);
    let quit = (0..7)
        .flat_map(|y| (0..80).map(move |x| (x, y)))
        .any(|(x, y)| app.hits.hit_at(x, y) == Some(&Hit::EditBlock(EditBlockButton::Quit)));
    assert!(quit, "Quit is still there");
}
