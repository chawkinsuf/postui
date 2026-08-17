use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::layout::PaneId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

#[test]
fn stage1_acceptance_flow() {
    let mut app = App::new_for_test();

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
    for _ in 0..40 {
        app.update(Action::Tick);
    }
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
