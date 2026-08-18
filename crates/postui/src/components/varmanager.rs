use crate::action::Action;
use crate::hit::HitMap;
use crate::paint::{fill, text};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// The title bar's height, matching the header/footer's 3-row painted
/// rhythm (a blank panel row, the content row, a blank panel row).
pub const TITLE_HEIGHT: u16 = 3;
/// The footer hint row's height: a single line, since the Manager already
/// carries its own title bar and doesn't need the app footer's blank
/// panel padding rows around it.
pub const HINT_HEIGHT: u16 = 1;

/// The full-frame Variable Manager screen (spec §5). This task delivers
/// only the shell: a title bar, an empty grid area, and a footer hint row.
/// The grid itself (rows, cells, editing) is Task 10.
#[derive(Debug, Default)]
pub struct VarManager;

impl VarManager {
    /// Handles a key while the Manager screen is open. `App::handle_key`
    /// routes every key here once an open modal and a modified global
    /// shortcut (e.g. ctrl+p for the palette) have had first refusal, and
    /// swallows anything this returns `None` for rather than falling
    /// through to the global keymap — so, for instance, plain `q` does not
    /// quit the app from this screen.
    ///
    /// `Esc` asks the app to leave the screen (`Action::CloseScreen`);
    /// nothing else is handled yet (grid navigation is Task 10).
    pub fn handle_key(&mut self, ev: KeyEvent) -> Option<Action> {
        match ev.code {
            KeyCode::Esc => Some(Action::CloseScreen),
            _ => None,
        }
    }

    /// Paints the shell into `area` (the full body rect between the app's
    /// header and footer): a `theme.panel` title bar reading "Variables —
    /// `<project>` · `<env>`", a `theme.page`-filled empty grid area, and a
    /// muted footer hint row.
    pub fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        project: &str,
        env: &str,
        _hits: &mut HitMap,
    ) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(TITLE_HEIGHT),
                Constraint::Min(0),
                Constraint::Length(HINT_HEIGHT),
            ])
            .split(area);
        let title_row = rows[0];
        let grid_row = rows[1];
        let hint_row = rows[2];

        let buf = frame.buffer_mut();

        fill(buf, title_row, theme.panel);
        if title_row.height > 0 {
            let mid_y = title_row.y + title_row.height / 2;
            let title = format!("Variables \u{2014} {project} \u{b7} {env}");
            text(
                buf,
                title_row.x + 3,
                mid_y,
                &title,
                theme.text,
                theme.panel,
                true,
            );
        }

        // Empty for now — Task 10 fills this in with the variable/env grid.
        fill(buf, grid_row, theme.page);

        fill(buf, hint_row, theme.panel);
        if hint_row.height > 0 {
            text(
                buf,
                hint_row.x + 1,
                hint_row.y,
                " esc back ",
                theme.text_muted,
                theme.panel,
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn render(project: &str, env: &str) -> String {
        let theme = Theme::for_terminal();
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut hits = HitMap::default();
        let vm = VarManager;
        terminal
            .draw(|f| vm.draw(f, f.area(), &theme, project, env, &mut hits))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn title_bar_shows_project_and_env() {
        let content = render("alpha", "qa");
        assert!(content.contains("Variables"));
        assert!(content.contains("alpha"));
        assert!(content.contains("qa"));
    }

    #[test]
    fn footer_hint_mentions_esc() {
        let content = render("alpha", "qa");
        assert!(content.contains("esc"));
        assert!(content.contains("back"));
    }

    #[test]
    fn esc_asks_the_app_to_close_the_screen() {
        let mut vm = VarManager;
        let ev = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev), Some(Action::CloseScreen));
    }

    #[test]
    fn unbound_plain_key_is_unhandled_here() {
        let mut vm = VarManager;
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(vm.handle_key(ev), None);
    }
}
