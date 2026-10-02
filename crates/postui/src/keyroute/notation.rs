//! The key notation the binding table, the key list and the tests share
//! (piece 4 spec §3.5): vim's, as in `dd`, `gg`, `<C-r>`, `<CR>`,
//! `<Down>`. One char is one key; `<…>` names a key or a ctrl chord.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key as the router sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    /// A printable char with shift folded in (`G`, `$`, `:`), or Space.
    Char(char),
    /// A ctrl chord on a letter: `<C-r>`.
    Ctrl(char),
    /// An unmodified named key: `<CR>`, `<Esc>`, `<Down>`.
    Named(KeyCode),
}

/// The `<…>` names, and the named keys a token can be.
const NAMES: &[(&str, KeyCode)] = &[
    ("CR", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("S-Tab", KeyCode::BackTab),
    ("BS", KeyCode::Backspace),
    ("Del", KeyCode::Delete),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
];

impl Tok {
    /// The token `ev` is, or `None` for a key no row can name: an alt
    /// chord, a ctrl chord on a non-letter, a modified named key. The
    /// router declines those as they are.
    pub fn of(ev: &KeyEvent) -> Option<Tok> {
        let mods = ev.modifiers.difference(KeyModifiers::SHIFT);
        match ev.code {
            KeyCode::Char(c) if mods.is_empty() => Some(Tok::Char(
                if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    c.to_ascii_uppercase()
                } else {
                    c
                },
            )),
            // Ctrl+shift+letter is a different chord (`KeyCombo::from_event`
            // sees it as the uppercase char with CONTROL, in both delivery
            // forms) and the notation has no `<C-S-x>`: no row names it.
            KeyCode::Char(c)
                if mods == KeyModifiers::CONTROL
                    && c.is_ascii_lowercase()
                    && !ev.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                Some(Tok::Ctrl(c))
            }
            // BackTab arrives with SHIFT from some terminals and without
            // it from others; it is one key either way.
            KeyCode::BackTab if mods.is_empty() => Some(Tok::Named(KeyCode::BackTab)),
            code if ev.modifiers.is_empty() && NAMES.iter().any(|(_, k)| *k == code) => {
                Some(Tok::Named(code))
            }
            _ => None,
        }
    }

    /// The key event that types this token: what the tests feed.
    pub fn event(self) -> KeyEvent {
        match self {
            Tok::Char(c) if c.is_ascii_uppercase() => {
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
            }
            Tok::Char(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            Tok::Ctrl(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
            Tok::Named(KeyCode::BackTab) => KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            Tok::Named(code) => KeyEvent::new(code, KeyModifiers::NONE),
        }
    }
}

/// Parses `notation` into tokens. Panics on an unknown `<…>` name: every
/// notation string is a literal in the table, the key list or a test.
pub fn parse(notation: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut rest = notation;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && end > 1
        {
            let name = &rest[1..end];
            out.push(match name {
                "Space" => Tok::Char(' '),
                "lt" => Tok::Char('<'),
                _ if name.len() == 3 && name.starts_with("C-") => {
                    Tok::Ctrl(name[2..].chars().next().expect("C-x").to_ascii_lowercase())
                }
                _ => Tok::Named(
                    NAMES
                        .iter()
                        .find(|(n, _)| *n == name)
                        .unwrap_or_else(|| panic!("unknown key <{name}> in {notation:?}"))
                        .1,
                ),
            });
            rest = &rest[end + 1..];
        } else {
            out.push(Tok::Char(c));
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The key events `notation` types, in order.
pub fn events(notation: &str) -> Vec<KeyEvent> {
    parse(notation).into_iter().map(Tok::event).collect()
}

/// How `notation` reads on a keycap: `<C-r>` is `^R`, `<CR>` is
/// `enter`, and a chord or named key before more keys is followed by a
/// space (`^W h`).
pub fn display(notation: &str) -> String {
    let toks = parse(notation);
    let mut out = String::new();
    for (i, t) in toks.iter().enumerate() {
        if i > 0 && !matches!(toks[i - 1], Tok::Char(_)) {
            out.push(' ');
        }
        match t {
            Tok::Char(' ') => out.push_str("space"),
            Tok::Char(c) => out.push(*c),
            Tok::Ctrl(c) => {
                out.push('^');
                out.push(c.to_ascii_uppercase());
            }
            Tok::Named(code) => out.push_str(&format!("{code:?}").to_lowercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation_round_trips_through_key_events() {
        for n in ["j", "gg", "G", "<C-r>", "<CR>", "<Esc>", "3j", "<Down>", ":", "$", "<S-Tab>", "<Space>", "<lt>"] {
            let toks = parse(n);
            let back: Vec<Tok> = events(n).iter().map(|e| Tok::of(e).expect("nameable")).collect();
            assert_eq!(back, toks, "{n}");
        }
        assert_eq!(parse("<C-w>h"), vec![Tok::Ctrl('w'), Tok::Char('h')]);
    }

    #[test]
    fn keys_no_row_can_name_are_none() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT)), None);
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL)), None);
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)), None);
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Char('R'), KeyModifiers::CONTROL)), None, "uppercase form");
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Char('r'), ctrl_shift)), None, "kitty form");
        assert_eq!(Tok::of(&KeyEvent::new(KeyCode::Char('R'), ctrl_shift)), None);
        assert_eq!(
            Tok::of(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(Tok::Ctrl('r')),
            "plain ctrl+r still names"
        );
        assert_eq!(
            Tok::of(&KeyEvent::new(KeyCode::Char('g'), KeyModifiers::SHIFT)),
            Some(Tok::Char('G')),
            "a shifted letter folds to its capital, as `KeyCombo::from_event` does"
        );
    }

    #[test]
    fn keycaps_read_as_the_footer_writes_them() {
        assert_eq!(display("<C-r>"), "^R");
        assert_eq!(display("u"), "u");
        assert_eq!(display("gg"), "gg");
        assert_eq!(display("<C-w>h"), "^W h");
        assert_eq!(display("<CR>"), "enter");
    }

    #[test]
    #[should_panic(expected = "unknown key <Nope>")]
    fn an_unknown_name_is_a_bug_in_a_literal() {
        parse("<Nope>");
    }
}
