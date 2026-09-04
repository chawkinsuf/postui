//! `postui --keydump`: raw input-event echo for terminal keyboard debugging.
//!
//! Prints exactly what crossterm event each keypress produces under a chosen
//! kitty-keyboard-protocol enhancement tier, so questions like "does cmd+Z
//! arrive in this terminal?" get answered by data instead of theory. A plain
//! `--keydump` pushes the same flags the app itself pushes
//! ([`crate::keys::app_enhancement_flags`]); `--keydump=N` pushes the raw
//! bitmask `N` (kitty protocol bits: 1 disambiguate, 2 event types,
//! 4 alternate keys, 8 all keys as escape codes; `0` pushes nothing), which
//! is how tier differences between terminals get isolated. Mouse capture is
//! also enabled, so clicks, drags, and scroll are echoed alongside keys.

use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyCode, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// One event's echo line. Key events also show the [`crate::keys::KeyCombo`]
/// the keymap would look up and, when the super-arrow fold rewrites the
/// event, what it becomes — so the dump shows both what the terminal sent
/// and what the app would act on.
pub fn describe(ev: &Event) -> String {
    match ev {
        Event::Key(k) => {
            let mut line = format!(
                "key    code={:?} mods={:?} kind={:?} state={:?}",
                k.code, k.modifiers, k.kind, k.state
            );
            let combo = crate::keys::KeyCombo::from_event(k);
            line.push_str(&format!(
                "  combo(code={:?} mods={:?})",
                combo.code, combo.modifiers
            ));
            let norm = crate::keys::normalize_super_keys(*k);
            if norm.code != k.code || norm.modifiers != k.modifiers {
                line.push_str(&format!(
                    "  super-fold->(code={:?} mods={:?})",
                    norm.code, norm.modifiers
                ));
            }
            line
        }
        // The pasted text itself may be huge or sensitive; the length is
        // what matters for debugging delivery.
        Event::Paste(text) => format!("paste  {} chars (bracketed)", text.chars().count()),
        Event::Mouse(m) => format!(
            "mouse  kind={:?} col={} row={} mods={:?}",
            m.kind, m.column, m.row, m.modifiers
        ),
        other => format!("{other:?}"),
    }
}

/// The bitmask actually pushed for a `flags` override (`--keydump=N`), or
/// the app's own tier for `None`. Unknown bits are dropped rather than
/// erroring so future protocol bits can be probed with older binaries.
pub fn resolve_flags(flags: Option<u8>) -> KeyboardEnhancementFlags {
    match flags {
        Some(bits) => KeyboardEnhancementFlags::from_bits_truncate(bits),
        None => crate::keys::app_enhancement_flags(),
    }
}

/// Runs the echo loop on the real terminal until ctrl+d. Raw mode plus
/// bracketed paste and mouse capture, and the enhancement push when the
/// resolved tier is non-empty.
pub fn run(flags: Option<u8>) -> anyhow::Result<()> {
    use std::io::Write;
    let resolved = resolve_flags(flags);
    let supported = matches!(
        ratatui::crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    println!("postui --keydump: raw key-event echo (ctrl+d exits); mouse events are reported too");
    println!(
        "TERM={:?} TERM_PROGRAM={:?}",
        std::env::var("TERM").ok(),
        std::env::var("TERM_PROGRAM").ok()
    );
    println!("terminal reports kitty keyboard protocol support: {supported}");
    println!(
        "pushing enhancement flags: {:?} (bits {})",
        resolved,
        resolved.bits()
    );

    enable_raw_mode()?;
    let mut out = std::io::stdout();
    let _ = execute!(out, EnableBracketedPaste);
    let _ = execute!(out, EnableMouseCapture);
    let pushed = !resolved.is_empty();
    if pushed {
        let _ = execute!(out, PushKeyboardEnhancementFlags(resolved));
    }

    let result: anyhow::Result<()> = (|| {
        loop {
            let ev = ratatui::crossterm::event::read()?;
            // Raw mode: no output post-processing, so lines need explicit \r.
            write!(std::io::stdout(), "{}\r\n", describe(&ev))?;
            std::io::stdout().flush()?;
            if let Event::Key(k) = ev
                && k.code == KeyCode::Char('d')
                && k.modifiers.contains(KeyModifiers::CONTROL)
            {
                return Ok(());
            }
        }
    })();

    if pushed {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

    #[test]
    fn resolve_flags_none_is_the_app_tier() {
        assert_eq!(resolve_flags(None), crate::keys::app_enhancement_flags());
    }

    #[test]
    fn resolve_flags_explicit_bits_and_zero() {
        assert_eq!(
            resolve_flags(Some(1)),
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        );
        assert_eq!(
            resolve_flags(Some(5)),
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        );
        assert!(resolve_flags(Some(0)).is_empty());
        // Unknown high bits are dropped, not an error.
        assert_eq!(
            resolve_flags(Some(0b1000_0001)),
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        );
    }

    #[test]
    fn describe_key_shows_code_mods_and_combo() {
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER));
        let line = describe(&ev);
        assert!(line.contains("Char('c')"), "{line}");
        assert!(line.contains("SUPER"), "{line}");
        assert!(line.contains("combo("), "{line}");
        assert!(
            !line.contains("super-fold"),
            "super over a non-arrow is not folded: {line}"
        );
    }

    #[test]
    fn describe_key_shows_the_super_arrow_fold() {
        let ev = Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER));
        let line = describe(&ev);
        assert!(line.contains("super-fold"), "{line}");
        assert!(line.contains("Home"), "{line}");
    }

    #[test]
    fn describe_mouse_shows_kind_col_row_mods() {
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 34,
            modifiers: KeyModifiers::SHIFT,
        });
        let line = describe(&ev);
        assert!(line.starts_with("mouse  "), "{line}");
        assert!(line.contains("kind=Down(Left)"), "{line}");
        assert!(line.contains("col=12"), "{line}");
        assert!(line.contains("row=34"), "{line}");
        assert!(line.contains("mods=KeyModifiers(SHIFT)"), "{line}");
    }

    #[test]
    fn describe_paste_reports_length_not_content() {
        let ev = Event::Paste("secret token".into());
        let line = describe(&ev);
        assert!(line.contains("12 chars"), "{line}");
        assert!(!line.contains("secret"), "{line}");
    }
}
