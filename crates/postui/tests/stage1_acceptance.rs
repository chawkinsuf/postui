use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::layout::PaneId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn render(app: &mut App) -> String {
    app.anims.finish_all();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

#[test]
fn stage1_acceptance_flow() {
    let mut app = App::new_for_test();
    // Modal-open settle (Task 13) is real-time-driven; this acceptance flow
    // asserts on modal content immediately after opening, so disable anims
    // for determinism (matches the app-level test convention elsewhere).
    app.anims.enabled = false;

    // 1. Initial frame: all chrome present, sidebar focused. Panes carry no
    // border/title of their own — the response pane's empty-state hint
    // identifies it instead.
    let frame = render(&mut app);
    assert!(frame.contains("REQUESTS") && frame.contains("response will appear here"));

    // 2. Focus cycling reaches every pane.
    app.update(Action::FocusNext);
    assert_eq!(app.focus, PaneId::Editor);
    app.update(Action::FocusNext);
    assert_eq!(app.focus, PaneId::Response);

    // 3. Toast renders and expires.
    app.update(Action::ShowToast(
        "Welcome to postui".into(),
        ToastKind::Info,
    ));
    assert!(render(&mut app).contains("Welcome to postui"));
    // A toast's lifetime is wall-clock (3s), not tick-counted (see
    // `components::toast`) -- real time is fine here, matching the
    // sleep-based settle precedent elsewhere in the app-level tests.
    std::thread::sleep(std::time::Duration::from_millis(3100));
    app.update(Action::Tick);
    assert!(!render(&mut app).contains("Welcome to postui"));

    // 4. Palette opens as a modal and renders.
    app.update(Action::OpenPalette);
    assert!(!app.modals.is_empty());
    assert!(render(&mut app).contains("Commands"));
    app.update(Action::Close);
    assert!(app.modals.is_empty());

    // 5. About modal via its action.
    app.update(Action::ShowAbout);
    assert!(render(&mut app).contains("local-first"));
    app.update(Action::Close);

    // 6. Quit.
    app.update(Action::Quit);
    assert!(app.should_quit);
}
