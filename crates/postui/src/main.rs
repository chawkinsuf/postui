use futures::StreamExt;
use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::keys::Keymap;
use postui::layout::{compute_layout, hit_test};
use postui::ui;
use ratatui::crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseButton,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminal = ratatui::init(); // installs a panic hook that restores the terminal
    enable_mouse_and_wrap_panic_hook();

    let result = run(&mut terminal).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Enables mouse capture and re-wraps the panic hook `ratatui::init()` just
/// installed: that hook restores the terminal but knows nothing about mouse
/// capture, so disable capture first and then delegate to it. Must be called
/// after *every* `ratatui::init()` — the external-editor round-trip re-inits,
/// which replaces the hook.
fn enable_mouse_and_wrap_panic_hook() {
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        prev_hook(info);
    }));
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
                ui::draw(frame, &mut app);
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
                            Event::Mouse(m) if app.modals.is_empty() => {
                                let size = terminal.size()?;
                                let layout =
                                    compute_layout(Rect::new(0, 0, size.width, size.height));
                                match m.kind {
                                    MouseEventKind::Down(MouseButton::Left) => {
                                        if let Some(pane) = hit_test(&layout, m.column, m.row) {
                                            redraw |= app.update(Action::FocusPane(pane));
                                        }
                                    }
                                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                                        if let Some(pane) = hit_test(&layout, m.column, m.row) {
                                            let d = if m.kind == MouseEventKind::ScrollUp { -3 } else { 3 };
                                            redraw |= app.update(Action::ScrollPane(pane, d));
                                        }
                                    }
                                    _ => {}
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

        // Actions that need the terminal suspended are parked by `App::update`
        // rather than applied there, so `update` stays pure and testable.
        // This is the one place allowed to tear the TUI down and rebuild it.
        if let Some(pending) = app.pending_terminal_action.take() {
            match pending {
                Action::OpenBodyInEditor => edit_body_externally(terminal, &mut app)?,
                other => debug_assert!(false, "not a terminal action: {other:?}"),
            }
            redraw = true;
        }
    }
    Ok(())
}

/// Hands the request body to `$EDITOR` (falling back to `vi`), then reads it
/// back. The TUI is fully torn down for the duration so the child editor owns
/// the terminal, and rebuilt afterwards. If the editor cannot be run or exits
/// non-zero the body is left untouched and the failure is toasted, because
/// silently discarding a half-written body would be the worst outcome here.
fn edit_body_externally(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> anyhow::Result<()> {
    use std::io::Write;

    let file = tempfile::Builder::new()
        .prefix("postui-body-")
        .suffix(".json")
        .tempfile()?;
    let path = file.path().to_path_buf();
    {
        let mut handle = file.as_file();
        handle.write_all(app.editor.body_text().as_bytes())?;
        handle.flush()?;
    }

    let command = std::env::var("EDITOR").unwrap_or_default();
    let command = if command.trim().is_empty() {
        "vi".to_string()
    } else {
        command
    };
    // `$EDITOR` conventionally may carry flags (e.g. `code -w`).
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("vi").to_string();
    let args: Vec<&str> = parts.collect();

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .status();

    *terminal = ratatui::init();
    enable_mouse_and_wrap_panic_hook();
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => match std::fs::read_to_string(&path) {
            // Editors conventionally leave a trailing newline; keeping it
            // would add a phantom blank line and a spurious dirty flag on
            // every round-trip.
            Ok(text) => {
                let text = text.strip_suffix('\n').unwrap_or(&text);
                let text = text.strip_suffix('\r').unwrap_or(text);
                app.editor.set_body_text(text);
            }
            Err(e) => {
                app.update(Action::ShowToast(
                    format!("could not read back the edited body: {e}"),
                    ToastKind::Error,
                ));
            }
        },
        Ok(s) => {
            app.update(Action::ShowToast(
                format!("{program} exited with {s}; body unchanged"),
                ToastKind::Error,
            ));
        }
        Err(e) => {
            app.update(Action::ShowToast(
                format!("could not run {program}: {e}"),
                ToastKind::Error,
            ));
        }
    }
    Ok(())
}
