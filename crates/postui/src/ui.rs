use crate::app::App;
use crate::components::{Component, DrawCtx};
use crate::layout::{compute_layout, PaneId};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, app: &App) {
    let layout = compute_layout(frame.area());
    crate::components::header_bar::draw_header(frame, layout.header, &app.theme);
    let ctx = |pane: PaneId| DrawCtx { theme: &app.theme, focused: app.focus == pane };
    app.sidebar.draw(frame, layout.sidebar, &ctx(PaneId::Sidebar));
    app.editor.draw(frame, layout.editor, &ctx(PaneId::Editor));
    app.response.draw(frame, layout.response, &ctx(PaneId::Response));
    crate::components::footer::draw_footer(frame, layout.footer, &app.theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App) -> String {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn full_frame_shows_all_panes_and_chrome() {
        let app = App::new();
        let content = render(&app);
        assert!(content.contains("Requests"));       // sidebar title
        assert!(content.contains("Request"));        // editor title
        assert!(content.contains("Response"));       // response title
        assert!(content.contains("postui"));         // header bar app name
        assert!(content.contains("No environment")); // header env selector placeholder
        assert!(content.contains("quit"));           // footer hint mentions quit key
        assert!(content.contains('╭'));              // rounded chrome
        assert!(content.contains("No project open")); // sidebar empty state
        assert!(content.contains("response will appear here")); // response empty state
    }
}
