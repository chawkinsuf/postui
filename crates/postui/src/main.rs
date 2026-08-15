mod action;
mod app;

use action::Action;
use app::App;
use futures::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let terminal = ratatui::init(); // installs a panic hook that restores the terminal
    let result = run(terminal).await;
    ratatui::restore();
    result
}

async fn run(mut terminal: ratatui::DefaultTerminal) -> anyhow::Result<()> {
    let mut app = App::new();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    while !app.should_quit {
        terminal.draw(|frame| {
            frame.render_widget(
                Line::from("postui — press q to quit"),
                frame.area(),
            );
        })?;

        tokio::select! {
            maybe_event = events.next() => {
                if let Some(Ok(event)) = maybe_event
                    && let Some(action) = map_event(&event) {
                    app.update(action);
                }
            }
            _ = tick.tick() => app.update(Action::Tick),
        }
    }
    Ok(())
}

fn map_event(event: &Event) -> Option<Action> {
    match event {
        Event::Key(KeyEvent { code: KeyCode::Char('q'), .. }) => Some(Action::Quit),
        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. })
            if modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}
