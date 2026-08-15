use crate::action::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        let parts: Vec<&str> = s.split('+').collect();
        let (mod_parts, key_part) = parts.split_at(parts.len() - 1);
        let mut modifiers = KeyModifiers::NONE;
        for m in mod_parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => return None,
            }
        }
        let key = key_part[0].to_ascii_lowercase();
        let code = match key.as_str() {
            "esc" => KeyCode::Esc,
            "enter" => KeyCode::Enter,
            "tab" if modifiers.contains(KeyModifiers::SHIFT) => {
                modifiers -= KeyModifiers::SHIFT; // terminals report shift+tab as BackTab
                KeyCode::BackTab
            }
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            k if k.len() >= 2 && k.len() <= 3 && k.starts_with('f') && k[1..].bytes().all(|b| b.is_ascii_digit()) => {
                match k[1..].parse::<u8>() {
                    Ok(n) if (1..=12).contains(&n) => KeyCode::F(n),
                    _ => return None,
                }
            }
            k => {
                let mut chars = k.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        // SHIFT on a printable char is implicit in the char itself.
                        modifiers -= KeyModifiers::SHIFT;
                        KeyCode::Char(c)
                    }
                    _ => return None,
                }
            }
        };
        Some(Self { code, modifiers })
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        // SHIFT is implicit in the char itself for printable keys.
        // Also strip SHIFT from BackTab since terminals deliver it with SHIFT still set.
        let mods = match ev.code {
            KeyCode::Char(_) | KeyCode::BackTab => ev.modifiers.difference(KeyModifiers::SHIFT),
            _ => ev.modifiers,
        };
        Self { code: ev.code, modifiers: mods }
    }
}

pub struct Keymap {
    bindings: HashMap<KeyCombo, Action>,
}

fn named_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("quit", Action::Quit),
        ("focus_next", Action::FocusNext),
        ("focus_prev", Action::FocusPrev),
        ("open_palette", Action::OpenPalette),
        ("close", Action::Close),
    ]
}

fn parse_overrides(toml_str: &str) -> anyhow::Result<Vec<(String, Vec<String>)>> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    let table: HashMap<String, OneOrMany> = toml::from_str(toml_str)?;
    Ok(table
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                match v {
                    OneOrMany::One(s) => vec![s],
                    OneOrMany::Many(v) => v,
                },
            )
        })
        .collect())
}

impl Keymap {
    pub fn default_bindings() -> Self {
        let defaults = [
            ("q", Action::Quit),
            ("ctrl+c", Action::Quit),
            ("tab", Action::FocusNext),
            ("shift+tab", Action::FocusPrev),
            ("ctrl+p", Action::OpenPalette),
            ("esc", Action::Close),
        ];
        let mut map = Self { bindings: HashMap::new() };
        for (s, a) in defaults {
            // Combos in this table are compile-time constants; parse cannot fail.
            if let Some(c) = KeyCombo::parse(s) {
                map.bind(c, a);
            }
        }
        map
    }

    pub fn lookup(&self, combo: &KeyCombo) -> Option<Action> {
        self.bindings.get(combo).cloned()
    }

    pub fn bind(&mut self, combo: KeyCombo, action: Action) {
        self.bindings.insert(combo, action);
    }

    pub fn apply_overrides(&mut self, toml_str: &str) -> anyhow::Result<()> {
        let ctrl_c = KeyCombo::parse("ctrl+c").unwrap();
        // Phase 1: validate the entire document before mutating any state, so a
        // late error can't leave earlier actions half-applied.
        let mut resolved: Vec<(Action, Vec<KeyCombo>)> = Vec::new();
        for (action_name, combo_strs) in parse_overrides(toml_str)? {
            let action = named_actions()
                .into_iter()
                .find(|(n, _)| *n == action_name)
                .map(|(_, a)| a)
                .ok_or_else(|| anyhow::anyhow!("unknown action: {action_name}"))?;
            let combos = combo_strs
                .iter()
                .map(|s| {
                    KeyCombo::parse(s).ok_or_else(|| anyhow::anyhow!("bad key combo: {s}"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if action != Action::Quit && combos.contains(&ctrl_c) {
                anyhow::bail!("ctrl+c is reserved for quit");
            }
            resolved.push((action, combos));
        }
        // Phase 2: everything validated, now apply.
        for (action, combos) in resolved {
            self.bindings.retain(|_, a| *a != action); // rebind removes old combo(s)
            for combo in combos {
                self.bind(combo, action.clone());
            }
        }
        // ctrl+c is always reserved for quit, regardless of overrides.
        self.bind(ctrl_c, Action::Quit);
        Ok(())
    }

    pub fn load() -> Self {
        let mut map = Self::default_bindings();
        if let Some(dirs) = directories::ProjectDirs::from("", "", postui_core::APP_NAME) {
            let path = dirs.config_dir().join("keys.toml");
            if let Ok(contents) = std::fs::read_to_string(path) {
                // Bad override files are ignored; surfaced as a toast in a later stage.
                let _ = map.apply_overrides(&contents);
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_char() {
        let c = KeyCombo::parse("q").unwrap();
        assert_eq!(c.code, KeyCode::Char('q'));
        assert_eq!(c.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parses_ctrl_combo_and_named_keys() {
        let c = KeyCombo::parse("ctrl+p").unwrap();
        assert_eq!(c.code, KeyCode::Char('p'));
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);
        assert_eq!(KeyCombo::parse("esc").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyCombo::parse("tab").unwrap().code, KeyCode::Tab);
        assert_eq!(KeyCombo::parse("shift+tab").unwrap().code, KeyCode::BackTab);
        assert_eq!(KeyCombo::parse("enter").unwrap().code, KeyCode::Enter);
        assert!(KeyCombo::parse("ctrl+bogus+q").is_none());
        assert!(KeyCombo::parse("").is_none());
    }

    #[test]
    fn default_bindings_cover_core_actions() {
        let m = Keymap::default_bindings();
        let get = |s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get("q"), Some(Action::Quit));
        assert_eq!(get("ctrl+c"), Some(Action::Quit));
        assert_eq!(get("tab"), Some(Action::FocusNext));
        assert_eq!(get("shift+tab"), Some(Action::FocusPrev));
        assert_eq!(get("ctrl+p"), Some(Action::OpenPalette));
        assert_eq!(get("esc"), Some(Action::Close));
    }

    #[test]
    fn override_accepts_string_or_list_and_replaces_all_defaults() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"quit = ["ctrl+q", "f10"]"#).unwrap();
        let get = |m: &Keymap, s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get(&m, "ctrl+q"), Some(Action::Quit));
        assert_eq!(get(&m, "q"), None, "default 'q' replaced by explicit list");
        m.apply_overrides(r#"open_palette = "ctrl+k""#).unwrap();
        assert_eq!(get(&m, "ctrl+k"), Some(Action::OpenPalette));
        assert_eq!(get(&m, "ctrl+p"), None);
    }

    #[test]
    fn ctrl_c_is_always_quit_and_cannot_be_taken() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"quit = "ctrl+q""#).unwrap();
        assert_eq!(m.lookup(&KeyCombo::parse("ctrl+c").unwrap()), Some(Action::Quit),
            "ctrl+c survives a quit rebind");
        assert!(m.apply_overrides(r#"open_palette = "ctrl+c""#).is_err(),
            "ctrl+c cannot be bound to another action");
    }

    #[test]
    fn unknown_action_and_bad_combo_still_error() {
        let mut m = Keymap::default_bindings();
        assert!(m.apply_overrides(r#"unknown_action = "x""#).is_err());
        assert!(m.apply_overrides(r#"quit = "not+a+key""#).is_err());
        assert!(m.apply_overrides(r#"quit = ["q", "not+a+key"]"#).is_err());
    }

    #[test]
    fn override_document_is_atomic_on_error() {
        let mut m = Keymap::default_bindings();
        let result = m.apply_overrides(
            r#"
            quit = "ctrl+q"
            open_palette = "not+a+key"
            "#,
        );
        assert!(result.is_err(), "a bad combo later in the doc must error");

        // Nothing should have been applied: defaults intact, no new combos present.
        let get = |s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get("q"), Some(Action::Quit), "default quit binding intact");
        assert_eq!(get("ctrl+c"), Some(Action::Quit), "ctrl+c still quit");
        assert_eq!(get("ctrl+p"), Some(Action::OpenPalette), "default palette binding intact");
        assert_eq!(get("ctrl+q"), None, "earlier valid rebind must not have been applied");
    }

    #[test]
    fn parses_function_keys() {
        for n in 1..=12u8 {
            let s = format!("f{n}");
            let c = KeyCombo::parse(&s).unwrap_or_else(|| panic!("failed to parse {s}"));
            assert_eq!(c.code, KeyCode::F(n));
        }
    }

    #[test]
    fn parse_edge_cases_return_none() {
        assert!(KeyCombo::parse("ctrl+").is_none());
        assert!(KeyCombo::parse("q+").is_none());
        assert!(KeyCombo::parse("qq").is_none());
    }

    #[test]
    fn from_event_strips_shift_on_chars() {
        let ev = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        assert_eq!(KeyCombo::from_event(&ev).modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn from_event_strips_shift_on_backtab() {
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let c = KeyCombo::from_event(&ev);
        assert_eq!(c.code, KeyCode::BackTab);
        assert_eq!(c.modifiers, KeyModifiers::NONE);
    }
}
