use futures::StreamExt;
use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::keys::Keymap;
use postui::layout::{compute_layout, hit_test};
use postui::ui;
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminal = ratatui::init(); // installs a panic hook that restores the terminal
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    // ratatui::init()'s panic hook restores the terminal but doesn't know about
    // mouse capture (enabled separately above), so wrap it: disable mouse capture
    // first, then delegate to ratatui's hook to do the rest of the restoration.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        prev_hook(info);
    }));

    let result = run(&mut terminal).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

async fn run(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Action>();
    let mut app = App::new(tx);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let keymap = Keymap::load();

    app.update(Action::ShowToast(
        "Welcome to postui".into(),
        ToastKind::Info,
    ));

    let mut redraw = true;
    while !app.should_quit {
        if redraw {
            terminal.draw(|frame| {
                ui::draw(frame, &app);
            })?;
            redraw = false;
        }

        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    None => {
                        // The event stream ended, which means stdin/the terminal input
                        // source is gone. There's nothing left to read events from, so
                        // continuing the loop would busy-spin forever re-selecting on an
                        // already-terminated stream. Quit cleanly instead.
                        app.should_quit = true;
                        redraw = true;
                    }
                    Some(Ok(event)) => {
                        match event {
                            Event::Key(ev) if ev.kind == KeyEventKind::Press => {
                                redraw |= app.handle_key(&keymap, ev);
                            }
                            Event::Mouse(MouseEvent {
                                kind: MouseEventKind::Down(MouseButton::Left),
                                column,
                                row,
                                ..
                            }) if app.modals.is_empty() => {
                                let size = terminal.size()?;
                                let layout =
                                    compute_layout(Rect::new(0, 0, size.width, size.height));
                                if let Some(pane) = hit_test(&layout, column, row) {
                                    redraw |= app.update(Action::FocusPane(pane));
                                }
                            }
                            Event::Resize(..) => {
                                redraw = true;
                            }
                            _ => {}
                        }
                    }
                    Some(Err(_)) => {}
                }
            }
            Some(action) = rx.recv() => {
                redraw |= app.update(action);
            }
            _ = tick.tick() => {
                redraw |= app.update(Action::Tick);
            }
        }
    }
    Ok(())
}
