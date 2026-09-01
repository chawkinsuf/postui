//! `postui --setup`: terminal keyboard-config guidance.
//!
//! macOS terminals ship "natural text editing" style keymaps that rewrite
//! option+arrow into `ESC b`/`ESC f` and cmd+arrow into `^A`/`^E` *before*
//! any keyboard protocol gets a say, and no escape sequence can override
//! those terminal-side maps (Ghostty's maintainers explicitly declined to
//! let the kitty protocol do so). postui therefore speaks those bytes
//! natively: alt+b/f are word motions, ^A/^E are line start/end, and
//! select-all lives on ctrl+shift+a — so arrows and carets work with NO
//! terminal config. What's left for this command is the optional extras:
//! cmd+a needs the terminal's own select-all unbound before it can reach
//! postui, and option+<letter> chords (opt+m, opt+1..) need the terminal's
//! option-as-alt switch. This module detects the terminal from its env
//! fingerprint and prints the matching snippet or instructions.

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
    "Ghostty (macOS): option+arrows, cmd+left/right, cmd+z/shift+z (undo/\n\
     redo) and cmd+c (copy a selection) already work in postui with no\n\
     config. Optional extras for ~/.config/ghostty/config (reload with\n\
     cmd+shift+,):\n\
     \n\
     \x20   # cmd+a = select all in postui (Ghostty's own select-all\n\
     \x20   # otherwise consumes the key; ctrl+shift+a works regardless)\n\
     \x20   keybind = super+a=unbind\n\
     \x20   # option+<letter> chords (opt+m, opt+1..) as alt, not composed chars\n\
     \x20   macos-option-as-alt = true\n"
}

fn kitty_section() -> &'static str {
    "kitty (macOS): option+arrows and cmd+left/right already work in\n\
     postui with no config. Optional extras for ~/.config/kitty/kitty.conf\n\
     (restart kitty after):\n\
     \n\
     \x20   # option+<letter> chords (opt+m, opt+1..) as alt, not composed chars\n\
     \x20   macos_option_as_alt yes\n\
     \n\
     If cmd+a is bound to a kitty action in your kitty.conf, remove that\n\
     line to let it reach postui as select-all (ctrl+shift+a works\n\
     regardless).\n"
}

fn iterm2_section() -> &'static str {
    "iTerm2: the \"Natural Text Editing\" key preset already works —\n\
     postui understands the bytes it sends for option+arrows (word jumps)\n\
     and cmd+left/right (line start/end). One optional setting, in\n\
     Settings > Profiles > Keys > General:\n\
     \n\
     \x20   set \"Left Option key\" to Esc+\n\
     \n\
     so option+<letter> chords (opt+m, opt+1..) arrive as alt instead of\n\
     composed characters.\n"
}

fn apple_terminal_section() -> &'static str {
    "Terminal.app cannot report the cmd key to terminal apps. postui's\n\
     built-in spellings cover the same ground: ctrl/option+arrows\n\
     word-jump, ctrl+a / ctrl+e jump to line start/end (Home/End —\n\
     fn+left/right — too), and selections come from shift+arrows or the\n\
     mouse (Terminal.app cannot carry ctrl+shift+a distinctly). In\n\
     Settings > Profiles > Keyboard, enable \"Use Option as Meta key\" so\n\
     option+<letter> chords (opt+m, opt+1..) arrive as alt.\n"
}

fn wezterm_section() -> &'static str {
    "WezTerm (macOS): add to ~/.wezterm.lua:\n\
     \n\
     \x20   config.send_composed_key_when_left_alt_is_pressed = false\n\
     \x20   config.enable_kitty_keyboard = true\n\
     \n\
     Option+<letter> chords then arrive as alt, and cmd keys WezTerm\n\
     itself leaves unbound are reported with the cmd modifier.\n"
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
        "\nEverywhere: option+arrows jump by word, cmd+left/right (and\n\
         ctrl+a / ctrl+e) jump to line start/end, ctrl+shift+a selects\n\
         all, cmd+z / cmd+shift+z undo/redo, and cmd+c copies a live\n\
         selection — no terminal config required for any of those where\n\
         the terminal itself leaves the keys alone.\n",
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
    fn ghostty_text_recommends_only_the_select_all_unbind_and_option_as_alt() {
        let t = setup_text(true, Some(TerminalKind::Ghostty));
        assert!(t.contains("Detected terminal: Ghostty"));
        assert!(t.contains("keybind = super+a=unbind"));
        assert!(t.contains("macos-option-as-alt = true"));
        assert!(
            !t.contains("arrow_left"),
            "arrow unbinds are gone: postui speaks the remapped bytes: {t}"
        );
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
        assert!(t.contains("Natural Text Editing"));
    }
}
