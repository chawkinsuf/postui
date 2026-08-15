use crate::app::App;
use crate::components::{Component, DrawCtx};
use crate::layout::{compute_layout, PaneId};
use ratatui::Frame;

/// Takes `&mut App` because components draw through `Component::draw(&mut
/// self, ..)`: the body editor's widget needs `&mut EditorState` to record the
/// viewport it was rendered into.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let layout = compute_layout(frame.area());
    let focus = app.focus;
    // Destructured so each component can be borrowed mutably alongside the
    // shared theme reference its DrawCtx holds.
    let App { theme, sidebar, editor, response, toasts, modals, .. } = app;
    crate::components::header_bar::draw_header(frame, layout.header, theme);
    let ctx = |pane: PaneId| DrawCtx { theme, focused: focus == pane };
    sidebar.draw(frame, layout.sidebar, &ctx(PaneId::Sidebar));
    editor.draw(frame, layout.editor, &ctx(PaneId::Editor));
    response.draw(frame, layout.response, &ctx(PaneId::Response));
    crate::components::footer::draw_footer(frame, layout.footer, theme, focus);
    toasts.draw(frame, frame.area(), theme);
    modals.draw(frame, frame.area(), theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn full_frame_shows_all_panes_and_chrome() {
        let mut app = App::new_for_test();
        let content = render(&mut app);
        assert!(content.contains("Requests"));       // sidebar title
        assert!(content.contains("Request"));        // editor title
        assert!(content.contains("Response"));       // response title
        assert!(content.contains("postui"));         // header bar app name
        assert!(content.contains("No environment")); // header env selector placeholder
        assert!(content.contains("quit"));           // footer hint mentions quit key
        assert!(content.contains('╭'));              // rounded chrome
        assert!(content.contains("No requests yet")); // sidebar empty state
        assert!(content.contains("response will appear here")); // response empty state
        assert!(content.contains("GET"));            // editor method badge (default method)
        assert!(content.contains("Params"));         // editor tab bar
        assert!(content.contains("Headers"));        // editor tab bar
        assert!(content.contains("Body"));           // editor tab bar
    }
}
