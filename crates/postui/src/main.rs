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
use layout::{compute_layout, hit_test};
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminal = ratatui::init(); // installs a panic hook that restores the terminal
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = run(&mut terminal).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
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
                if let Some(Ok(event)) = maybe_event {
                    match event {
                        Event::Key(ev) if ev.kind == KeyEventKind::Press => {
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
                        Event::Mouse(MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            column,
                            row,
                            ..
                        }) if app.modals.is_empty() => {
                            let size = terminal.size()?;
                            let layout = compute_layout(ratatui::layout::Rect::new(0, 0, size.width, size.height));
                            if let Some(pane) = hit_test(&layout, column, row) {
                                app.update(Action::FocusPane(pane));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ = tick.tick() => app.update(Action::Tick),
        }
    }
    Ok(())
}
