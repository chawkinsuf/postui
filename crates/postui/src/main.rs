use futures::StreamExt;
use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::keys::Keymap;
use postui::ui;
use ratatui::crossterm::SynchronizedUpdate;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EndSynchronizedUpdate, SetTitle};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = postui::config::parse_cli(std::env::args().nth(1));
    let cli_root = match cli {
        postui::config::CliParse::Usage => {
            println!("usage: postui [directory]");
            println!("       postui --setup    terminal keyboard setup (macOS opt/cmd keys)");
            println!(
                "       postui --keydump[=BITS]  echo raw key events (kitty flag bits; 0=none)"
            );
            return Ok(());
        }
        postui::config::CliParse::Keydump { flags } => {
            return postui::keydump::run(flags);
        }
        postui::config::CliParse::Setup => {
            print!(
                "{}",
                postui::setup::setup_text(
                    cfg!(target_os = "macos"),
                    postui::setup::detect_from_env()
                )
            );
            return Ok(());
        }
        postui::config::CliParse::Root(root) => root,
    };

    let mut terminal = ratatui::init(); // installs a panic hook that restores the terminal
    enable_mouse_and_wrap_panic_hook();
    // The app name lives in the window title, not the header bar.
    let _ = execute!(std::io::stdout(), SetTitle("postui"));

    let result = run(&mut terminal, cli_root).await;
    reset_pointer_shape();
    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste
    );
    drain_pending_input();
    ratatui::restore();
    result
}

/// Swallows input the terminal emitted between the app's last read and its
/// processing of `DisableMouseCapture` — mostly mouse-motion reports, since
/// the pointer is usually moving right as the user quits. Left in the tty
/// buffer, those bytes would be typed at whatever reads stdin next (the
/// shell after exit, `$EDITOR` during a round-trip) as `35;196;23M` garbage.
/// Must run while raw mode is still on, i.e. before `ratatui::restore()`.
/// The short poll gives the terminal time to process the disable; the
/// deadline caps the wait in case reports keep streaming in.
///
/// `poll` returning `Ok(false)` before its timeout elapsed is not "quiet":
/// crossterm maps a waker interrupt to `Ok(false)`, and `EventStream::drop`
/// (as `run` returns) fires that waker with no reader waiting, so the stale
/// wake surfaces here as an instant empty poll. Only a timeout that actually
/// ran its full course means the buffer has gone quiet.
fn drain_pending_input() {
    use ratatui::crossterm::event::{poll, read};
    const QUIET: Duration = Duration::from_millis(25);
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    while std::time::Instant::now() < deadline {
        let started = std::time::Instant::now();
        match poll(QUIET) {
            Ok(true) => {
                let _ = read();
            }
            Ok(false) if started.elapsed() < QUIET => continue,
            _ => break,
        }
    }
}

/// Writes a Kitty pointer-shape hint (OSC 22, task 8d): `\x1b]22;{shape}\x07`,
/// BEL-terminated exactly as Textual's own writer uses. Ignored outright by
/// terminals that don't support the protocol, so this has no fallback path
/// to maintain.
fn write_pointer_shape(shape: postui::hit::PointerShape) {
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x1b]22;{}\x07", shape.as_str());
}

/// Resets the pointer to the terminal's own default shape. Called from the
/// normal shutdown path and the panic hook, mirroring their existing
/// `let _ = execute!/write` restore pattern — a crash must not leave a
/// hand cursor hanging over whatever the terminal shows next.
fn reset_pointer_shape() {
    write_pointer_shape(postui::hit::PointerShape::Default);
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

/// Enables mouse capture and re-wraps the panic hook `ratatui::init()` just
/// installed: that hook restores the terminal but knows nothing about mouse
/// capture, so disable capture first and then delegate to it. Must be called
/// after *every* `ratatui::init()` — the external-editor round-trip re-inits,
/// which replaces the hook. Also enables focus-change reporting, which
/// drives `Action::ReloadProjectFiles` on `Event::FocusGained`.
fn enable_mouse_and_wrap_panic_hook() {
    // Bracketed paste makes a terminal-level paste (cmd+V on macOS,
    // ctrl+shift+V on Linux) arrive as one `Event::Paste` instead of a
    // flood of synthetic keypresses — without it, any newline in the
    // pasted text acts as a real Enter (committing cells, firing modals).
    let _ = execute!(
        std::io::stdout(),
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste
    );
    // Ctrl+Shift+Z is only distinguishable from Ctrl+Z with the kitty
    // keyboard protocol; without it Ctrl+Y is the redo binding that works.
    // The tier pushed lives in `keys::app_enhancement_flags` (shared with
    // `--keydump` so the diagnostic reproduces the app's input conditions).
    if matches!(
        ratatui::crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) {
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(postui::keys::app_enhancement_flags())
        );
    }
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // `stdout().sync_update(...)` around `terminal.draw` (main.rs's
        // event loop) sends EndSynchronizedUpdate only when its closure
        // *returns* — a panic inside `terminal.draw` unwinds straight past
        // that, so the terminal is left in synchronized-update mode with
        // no other code path to clear it. Neither `ratatui::restore()` nor
        // the rest of this hook's own escape sequences touch that mode, so
        // without this the terminal would come back from a draw panic
        // frozen on its last synced frame. Cleared first, before the other
        // restores, on the same reasoning as those: nothing else in this
        // hook depends on sync mode being on or off, so order among them
        // doesn't matter, but doing it up front means every restore that
        // follows is unambiguously outside a synchronized update.
        let _ = execute!(std::io::stdout(), EndSynchronizedUpdate);
        reset_pointer_shape();
        let _ = execute!(
            std::io::stdout(),
            PopKeyboardEnhancementFlags,
            DisableMouseCapture,
            DisableFocusChange,
            DisableBracketedPaste
        );
        prev_hook(info);
    }));
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    cli_root: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Action>();
    let mut app = App::new(tx, cli_root);
    // Same probe `enable_mouse_and_wrap_panic_hook` keys the enhancement
    // push off: where the terminal can report Shift+Enter, advertise it as
    // the send key; elsewhere the footer keeps showing ^R.
    app.shift_enter_send = matches!(
        ratatui::crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    let mut events = EventStream::new();
    let keymap = Keymap::load();

    app.update(Action::ShowToast(
        "Welcome to postui".into(),
        ToastKind::Info,
    ));

    let mut redraw = true;
    while !app.should_quit {
        if redraw {
            // Wrapped in a synchronized-update guard (crossterm's BSU/ESU,
            // `\x1b[?2026h`/`l`): without it, a terminal that samples the
            // screen mid-write can paint a partially-applied frame — the
            // multi-cell regions an animation repaints every tick (like
            // the list-travel band) are exactly where that tearing shows.
            // Terminals that don't support the mode simply ignore the
            // escape sequences, so this is a no-op fallback everywhere
            // else. Writes straight to stdout rather than through the
            // backend's own writer: both land on the same fd, and BSU is
            // flushed (queued then `execute!`d) before `terminal.draw`
            // writes anything, with ESU flushed only after `draw`
            // returns, so ordering on the single-threaded fd is preserved
            // either way.
            std::io::stdout().sync_update(|_| -> anyhow::Result<()> {
                terminal.draw(|frame| {
                    ui::draw(frame, &mut app);
                })?;
                // Kitty pointer-shape hint (task 8d): piggybacks on the hover
                // state `ui::draw` just styled from, and only writes when it
                // changed. Harmless inside the synchronized-update window —
                // OSC 22 has no effect on layout — and keeping it there means
                // the hint lands atomically with the frame it matches.
                if let Some(shape) = app.pointer_shape_update() {
                    write_pointer_shape(shape);
                }
                Ok(())
            })??;
            // The frame may have added or removed a control under a resting
            // pointer (hover-revealed buttons, view switches); re-resolve
            // hover against the fresh hit map and repaint if it changed.
            redraw = app.resync_hover();
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
                            Event::Mouse(m) => {
                                redraw |= app.handle_mouse(m);
                            }
                            Event::Resize(..) => {
                                redraw = true;
                            }
                            Event::FocusGained => {
                                redraw |= app.update(Action::ReloadProjectFiles);
                            }
                            // A terminal-level paste (bracketed): the whole
                            // text in one event, routed to the live caret.
                            Event::Paste(text) => {
                                redraw |= app.paste_text(&text);
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
            // Adaptive tick period: ~60fps while an animation is easing so
            // motion reads as smooth (a short, eased move sampled at 33ms
            // could collapse into one or two visible steps — task 8e's
            // list-travel flicker), ~10fps the rest of the time so an
            // idle app costs almost nothing.
            _ = tokio::time::sleep(if app.animating() {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(100)
            }) => {
                redraw |= app.update(Action::Tick);
            }
        }

        // Actions that need the terminal suspended are parked by `App::update`
        // rather than applied there, so `update` stays pure and testable.
        // This is the one place allowed to tear the TUI down and rebuild it.
        if let Some(pending) = app.pending_terminal_action.take() {
            match pending {
                Action::OpenBodyInEditor => edit_body_externally(terminal, &mut app)?,
                Action::OpenResponseInEditor => view_response_externally(terminal, &mut app)?,
                other => debug_assert!(false, "not a terminal action: {other:?}"),
            }
            redraw = true;
        }

        // Diffs the open request against its shadow copy once per loop
        // iteration, after both the event branch and the terminal-action
        // branch have had their chance to mutate it — the one place
        // keystroke- and mouse-path edits get captured as undo steps.
        redraw |= app.capture_undo();
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
    let path = write_editor_tempfile("postui-body-", ".json", &app.editor.body_text())?;
    run_editor_and_restore(terminal, app, &path, true)
}

/// Hands the response pane's active tab's text to `$EDITOR`, view-only:
/// the temp file is discarded on exit, nothing is read back. The tab's
/// text (pretty / raw / headers) and the file extension both follow the
/// active view, exactly like the toolbar's copy and save.
fn view_response_externally(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> anyhow::Result<()> {
    let Some(view) = app.session.response.view() else {
        return Ok(()); // gated in App::update; races to nothing here
    };
    let is_json_body = view.mode != postui::components::response::ViewMode::Headers
        && matches!(
            app.session.response.state(),
            postui::components::response::ResponseState::Ready(d)
                if d.content_type.as_deref().is_some_and(|c| c.contains("json"))
        );
    let suffix = if is_json_body { ".json" } else { ".txt" };
    let path = write_editor_tempfile("postui-response-", suffix, &view.view_text())?;
    run_editor_and_restore(terminal, app, &path, false)
}

/// Writes `text` to a fresh temp file for an external-editor round-trip and
/// keeps the file alive (into_temp_path is dropped by the caller when the
/// editor exits — `run_editor_and_restore` removes it explicitly instead so
/// the path stays valid for the child's whole lifetime).
fn write_editor_tempfile(
    prefix: &str,
    suffix: &str,
    text: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use std::io::Write;
    let file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()?;
    let (mut handle, path) = file.keep()?;
    handle.write_all(text.as_bytes())?;
    handle.flush()?;
    Ok(path)
}

/// Tears the TUI down, runs `$EDITOR` (falling back to `vi`) on `path`,
/// rebuilds the TUI, then — only when `read_back` — feeds the edited text
/// back into the request body. The temp file is removed either way.
fn run_editor_and_restore(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    path: &std::path::Path,
    read_back: bool,
) -> anyhow::Result<()> {
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

    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste
    );
    drain_pending_input();
    ratatui::restore();

    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(path)
        .status();

    *terminal = ratatui::init();
    enable_mouse_and_wrap_panic_hook();
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => {
            if read_back {
                match std::fs::read_to_string(path) {
                    // Editors conventionally leave a trailing newline;
                    // keeping it would add a phantom blank line and a
                    // spurious dirty flag on every round-trip.
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
                }
            }
        }
        Ok(s) => {
            app.update(Action::ShowToast(
                format!(
                    "{program} exited with {s}{}",
                    if read_back { "; body unchanged" } else { "" }
                ),
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
    let _ = std::fs::remove_file(path);
    Ok(())
}
