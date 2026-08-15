mod action;
mod app;
mod keys;
mod theme;
mod layout;
mod components;
mod ui;

use action::Action;
use app::App;
use futures::StreamExt;
use keys::{KeyCombo, Keymap};
use ratatui::crossterm::event::{Event, EventStream, KeyEventKind};
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
    let keymap = Keymap::load();

    while !app.should_quit {
        terminal.draw(|frame| {
            ui::draw(frame, &app);
        })?;

        tokio::select! {
            maybe_event = events.next() => {
                if let Some(Ok(Event::Key(ev))) = maybe_event
                    && ev.kind == KeyEventKind::Press
                {
                    let action = if !app.modals.is_empty() {
                        app.modals.handle_key(ev)
                    } else {
                        keymap.lookup(&KeyCombo::from_event(&ev))
                    };
                    if let Some(action) = action {
                        if !app.modals.is_empty() && action != Action::Close {
                            let _ = app.modals.pop();
                        }
                        app.update(action);
                    }
                }
            }
            _ = tick.tick() => app.update(Action::Tick),
        }
    }
    Ok(())
}
