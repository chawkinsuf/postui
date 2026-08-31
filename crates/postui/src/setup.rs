//! `postui --setup`: terminal keyboard-config guidance.
//!
//! macOS terminals ship "natural text editing" style keymaps that rewrite
//! option+arrow into `ESC b`/`ESC f` and cmd+arrow into `^A`/`^E` *before*
//! any keyboard protocol gets a say — bytes postui cannot tell apart from
//! real alt+b / ctrl+a chords. No escape sequence can override those
//! terminal-side maps (Ghostty's maintainers explicitly declined to let the
//! kitty protocol do so), so the fix is one-time terminal config. This
//! module detects the terminal from its env fingerprint and prints the
//! matching snippet or instructions.

/// Terminals `--setup` has tailored guidance for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    Ghostty,
    Kitty,
    ITerm2,
    AppleTerminal,
    WezTerm,
}

impl TerminalKind {
    fn name(self) -> &'static str {
        match self {
            Self::Ghostty => "Ghostty",
            Self::Kitty => "kitty",
            Self::ITerm2 => "iTerm2",
            Self::AppleTerminal => "Terminal.app",
            Self::WezTerm => "WezTerm",
        }
    }
}

/// Identifies the hosting terminal from `TERM_PROGRAM` (primary — every
/// terminal here except kitty sets it), falling back to `TERM` (kitty and
/// Ghostty install their own terminfo names) and then `LC_TERMINAL`
/// (iTerm2's fingerprint that survives SSH, unlike `TERM_PROGRAM`).
/// Best-effort: inside tmux `TERM_PROGRAM` is usually absent and `TERM`
/// is `tmux-256color`, so detection can miss — `setup_text` then prints
/// every section.
pub fn detect(
    term_program: Option<&str>,
    term: Option<&str>,
    lc_terminal: Option<&str>,
) -> Option<TerminalKind> {
    match term_program {
        Some("ghostty") => return Some(TerminalKind::Ghostty),
        Some("iTerm.app") => return Some(TerminalKind::ITerm2),
        Some("Apple_Terminal") => return Some(TerminalKind::AppleTerminal),
        Some("WezTerm") => return Some(TerminalKind::WezTerm),
        _ => {}
    }
    match term {
        Some("xterm-kitty") => return Some(TerminalKind::Kitty),
        Some("xterm-ghostty") => return Some(TerminalKind::Ghostty),
        _ => {}
    }
    match lc_terminal {
        Some("iTerm2") => Some(TerminalKind::ITerm2),
        _ => None,
    }
}

/// Reads the env fingerprint `detect` wants from the real environment.
pub fn detect_from_env() -> Option<TerminalKind> {
    let term_program = std::env::var("TERM_PROGRAM").ok();
    let term = std::env::var("TERM").ok();
    let lc_terminal = std::env::var("LC_TERMINAL").ok();
    detect(
        term_program.as_deref(),
        term.as_deref(),
        lc_terminal.as_deref(),
    )
}

fn ghostty_section() -> &'static str {
    "Ghostty (macOS) remaps these keys to legacy Terminal.app bytes before\n\
     apps can see them. Add to ~/.config/ghostty/config and reload the\n\
     config (cmd+shift+,):\n\
     \n\
     \x20   # let postui see real option+arrow / cmd+arrow keys\n\
     \x20   keybind = alt+arrow_left=unbind\n\
     \x20   keybind = alt+arrow_right=unbind\n\
     \x20   keybind = super+arrow_left=unbind\n\
     \x20   keybind = super+arrow_right=unbind\n\
     \x20   macos-option-as-alt = true\n"
}

fn kitty_section() -> &'static str {
    "kitty (macOS): add to ~/.config/kitty/kitty.conf and restart kitty:\n\
     \n\
     \x20   macos_option_as_alt yes\n\
     \n\
     kitty reports unmapped cmd+arrow keys to apps on its own; if you have\n\
     bound cmd+arrows to kitty actions in kitty.conf, remove those lines.\n"
}

fn iterm2_section() -> &'static str {
    "iTerm2: profile key mappings intercept these keys (the \"Natural Text\n\
     Editing\" preset maps option+arrows to ESC b/f and cmd+arrows to ^A/^E).\n\
     In Settings > Profiles > Keys:\n\
     \n\
     \x20   1. Under Key Mappings, delete the entries for opt+left, opt+right,\n\
     \x20      cmd+left and cmd+right (or switch away from the Natural Text\n\
     \x20      Editing preset).\n\
     \x20   2. Under General, set \"Left Option key\" to Esc+.\n\
     \n\
     Option+arrows then reach postui as real alt+arrows. Cmd+arrow reporting\n\
     depends on your iTerm2 version's kitty-keyboard-protocol support; if\n\
     cmd+arrows do nothing afterwards, Home/End (fn+left/right) do the same.\n"
}

fn apple_terminal_section() -> &'static str {
    "Terminal.app cannot report the cmd key to terminal apps and does not\n\
     support the keyboard protocol that carries modified arrows reliably.\n\
     Inside it, use postui's built-in equivalents instead: ctrl+arrows\n\
     word-jump, Home/End (fn+left/right) jump to line start/end, and\n\
     ctrl+a selects all.\n"
}

fn wezterm_section() -> &'static str {
    "WezTerm (macOS): add to ~/.wezterm.lua:\n\
     \n\
     \x20   config.send_composed_key_when_left_alt_is_pressed = false\n\
     \x20   config.enable_kitty_keyboard = true\n\
     \n\
     Option+arrows then arrive as alt+arrows and cmd+arrows are reported\n\
     with the cmd modifier.\n"
}

fn section(kind: TerminalKind) -> &'static str {
    match kind {
        TerminalKind::Ghostty => ghostty_section(),
        TerminalKind::Kitty => kitty_section(),
        TerminalKind::ITerm2 => iterm2_section(),
        TerminalKind::AppleTerminal => apple_terminal_section(),
        TerminalKind::WezTerm => wezterm_section(),
    }
}

/// The full `--setup` output: what was detected, the matching guidance (or
/// every section when detection failed), and what the payoff is. `macos`
/// is the OS this binary was built for (`cfg!(target_os = "macos")`) — on
/// Linux the remaps this command fixes don't exist (Ghostty and friends
/// ship them only in their macOS builds), so the output short-circuits,
/// except when the env fingerprint says a mac terminal is on the far end
/// of an SSH session.
pub fn setup_text(macos: bool, kind: Option<TerminalKind>) -> String {
    let mut out = String::from("postui --setup: terminal keyboard setup\n\n");
    if !macos {
        out.push_str(
            "Nothing to configure on Linux: terminals here deliver alt+arrows\n\
             and ctrl+arrows to postui unmodified — the option/cmd remapping\n\
             this command fixes ships only in terminals' macOS builds.\n",
        );
        // Only iTerm2 proves a Mac on the far end of SSH: it has no Linux
        // build and its LC_TERMINAL crosses the connection. Ghostty/kitty
        // fingerprints on a Linux binary are just their (remap-free) Linux
        // builds running locally.
        if kind == Some(TerminalKind::ITerm2) {
            out.push_str(
                "\nBut this looks like an SSH session from a Mac running iTerm2,\n\
                 whose mac-side remaps still apply. Its guidance:\n\n",
            );
            out.push_str(section(TerminalKind::ITerm2));
        }
        return out;
    }
    match kind {
        Some(k) => {
            out.push_str(&format!("Detected terminal: {}\n\n", k.name()));
            out.push_str(section(k));
        }
        None => {
            out.push_str(
                "Could not detect your terminal (inside tmux, run this from a\n\
                 pane outside tmux). Guidance for every supported terminal:\n",
            );
            for k in [
                TerminalKind::Ghostty,
                TerminalKind::Kitty,
                TerminalKind::ITerm2,
                TerminalKind::AppleTerminal,
                TerminalKind::WezTerm,
            ] {
                out.push_str(&format!("\n--- {} ---\n", k.name()));
                out.push_str(section(k));
            }
        }
    }
    out.push_str(
        "\nAfter this, in postui text fields option+arrows jump by word,\n\
         cmd+arrows jump to line start/end (cmd+shift+arrows select there),\n\
         and cmd+up/down jump to the start/end of the body.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_prefers_term_program() {
        assert_eq!(
            detect(Some("ghostty"), Some("xterm-256color"), None),
            Some(TerminalKind::Ghostty)
        );
        assert_eq!(
            detect(Some("iTerm.app"), None, None),
            Some(TerminalKind::ITerm2)
        );
        assert_eq!(
            detect(Some("Apple_Terminal"), None, None),
            Some(TerminalKind::AppleTerminal)
        );
        assert_eq!(
            detect(Some("WezTerm"), None, None),
            Some(TerminalKind::WezTerm)
        );
    }

    #[test]
    fn detect_falls_back_to_term_then_lc_terminal() {
        assert_eq!(
            detect(None, Some("xterm-kitty"), None),
            Some(TerminalKind::Kitty)
        );
        assert_eq!(
            detect(None, Some("xterm-ghostty"), None),
            Some(TerminalKind::Ghostty)
        );
        // iTerm2 over SSH: TERM_PROGRAM is gone, LC_TERMINAL crossed.
        assert_eq!(
            detect(None, Some("xterm-256color"), Some("iTerm2")),
            Some(TerminalKind::ITerm2)
        );
        assert_eq!(detect(None, Some("tmux-256color"), None), None);
        assert_eq!(detect(None, None, None), None);
    }

    #[test]
    fn ghostty_text_has_the_unbind_snippet() {
        let t = setup_text(true, Some(TerminalKind::Ghostty));
        assert!(t.contains("Detected terminal: Ghostty"));
        assert!(t.contains("keybind = alt+arrow_left=unbind"));
        assert!(t.contains("keybind = super+arrow_right=unbind"));
        assert!(t.contains("macos-option-as-alt = true"));
    }

    #[test]
    fn unknown_terminal_prints_every_section() {
        let t = setup_text(true, None);
        for name in ["Ghostty", "kitty", "iTerm2", "Terminal.app", "WezTerm"] {
            assert!(t.contains(&format!("--- {name} ---")), "missing {name}");
        }
    }

    #[test]
    fn linux_short_circuits_even_with_a_local_terminal_detected() {
        let t = setup_text(false, Some(TerminalKind::Ghostty));
        assert!(t.contains("Nothing to configure on Linux"));
        assert!(
            !t.contains("keybind ="),
            "local Linux Ghostty needs no snippet: {t}"
        );
    }

    #[test]
    fn linux_over_ssh_from_iterm2_prints_the_mac_guidance() {
        let t = setup_text(false, Some(TerminalKind::ITerm2));
        assert!(t.contains("Nothing to configure on Linux"));
        assert!(t.contains("SSH session from a Mac running iTerm2"));
        assert!(t.contains("Natural Text\nEditing"));
    }
}
