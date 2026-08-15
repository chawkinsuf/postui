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
        let mut bindings = HashMap::new();
        for (s, a) in defaults {
            // Combos in this table are compile-time constants; parse cannot fail.
            if let Some(c) = KeyCombo::parse(s) {
                bindings.insert(c, a);
            }
        }
        Self { bindings }
    }

    pub fn lookup(&self, combo: &KeyCombo) -> Option<Action> {
        self.bindings.get(combo).cloned()
    }

    pub fn apply_overrides(&mut self, toml_str: &str) -> anyhow::Result<()> {
        let table: HashMap<String, String> = toml::from_str(toml_str)?;
        for (action_name, combo_str) in table {
            let action = named_actions()
                .into_iter()
                .find(|(n, _)| *n == action_name)
                .map(|(_, a)| a)
                .ok_or_else(|| anyhow::anyhow!("unknown action: {action_name}"))?;
            let combo = KeyCombo::parse(&combo_str)
                .ok_or_else(|| anyhow::anyhow!("bad key combo: {combo_str}"))?;
            self.bindings.retain(|_, a| *a != action); // rebind removes old combo
            self.bindings.insert(combo, action);
        }
        Ok(())
    }

    #[allow(dead_code)]
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
    fn toml_overrides_rebind_and_reject_unknown() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"
            quit = "ctrl+q"
            open_palette = "ctrl+k"
        "#).unwrap();
        let get = |s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get("ctrl+q"), Some(Action::Quit));
        assert_eq!(get("ctrl+k"), Some(Action::OpenPalette));
        assert_eq!(get("ctrl+p"), None, "old binding removed on rebind");
        assert!(m.apply_overrides(r#"unknown_action = "x""#).is_err());
        assert!(m.apply_overrides(r#"quit = "not+a+key""#).is_err());
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
