use crate::app::App;
use crate::components::{Component, DrawCtx};
use crate::hit::Hit;
use crate::layout::{PaneId, compute_layout};
use ratatui::Frame;

/// Takes `&mut App` because components draw through `Component::draw(&mut
/// self, ..)`: the body editor's widget needs `&mut EditorState` to record the
/// viewport it was rendered into.
///
/// The `HitMap` is rebuilt every frame: taken out of `app` up front (so it
/// can be threaded through each draw call as an independent `&mut` borrow
/// alongside `app`'s other fields), cleared, and put back at the end. Pane
/// rects are registered first so any hit a component registers later (a
/// button, a row) is topmost at that point per [`HitMap::hit_at`]'s
/// last-registered-wins rule.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let layout = compute_layout(frame.area());
    let focus = app.focus;
    let mut hits = std::mem::take(&mut app.hits);
    hits.clear();
    hits.register(layout.sidebar, Hit::Pane(PaneId::Sidebar));
    hits.register(layout.editor, Hit::Pane(PaneId::Editor));
    hits.register(layout.response, Hit::Pane(PaneId::Response));
    let hovered = app.hovered.as_ref();
    // Destructured so each component can be borrowed mutably alongside the
    // shared theme reference its DrawCtx holds.
    let App {
        theme,
        sidebar,
        editor,
        response,
        toasts,
        modals,
        ..
    } = app;
    crate::components::header_bar::draw_header(
        frame,
        layout.header,
        theme,
        &app.project.display_name(),
        &app.project.env_label(),
        &mut hits,
        hovered,
    );
    let ctx = |pane: PaneId| DrawCtx {
        theme,
        focused: focus == pane,
        hovered,
    };
    sidebar.draw(frame, layout.sidebar, &ctx(PaneId::Sidebar), &mut hits);
    editor.draw(frame, layout.editor, &ctx(PaneId::Editor), &mut hits);
    response.draw(frame, layout.response, &ctx(PaneId::Response), &mut hits);
    crate::components::footer::draw_footer(frame, layout.footer, theme, focus, &mut hits, hovered);
    toasts.draw(frame, frame.area(), theme);
    modals.draw(frame, frame.area(), theme, &mut hits);
    app.hits = hits;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
        assert!(content.contains("Requests")); // sidebar title
        assert!(content.contains("Request")); // editor title
        assert!(content.contains("Response")); // response title
        assert!(content.contains("postui")); // header bar app name
        assert!(content.contains("no env")); // header env selector placeholder
        assert!(content.contains("quit")); // footer hint mentions quit key
        assert!(content.contains('╭')); // rounded chrome
        assert!(content.contains("No requests yet")); // sidebar empty state
        assert!(content.contains("response will appear here")); // response empty state
        assert!(content.contains("GET")); // editor method badge (default method)
        assert!(content.contains("Params")); // editor tab bar
        assert!(content.contains("Headers")); // editor tab bar
        assert!(content.contains("Body")); // editor tab bar
    }
}
