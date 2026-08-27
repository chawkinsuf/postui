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
            k if k.len() >= 2
                && k.len() <= 3
                && k.starts_with('f')
                && k[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                match k[1..].parse::<u8>() {
                    Ok(n) if (1..=12).contains(&n) => KeyCode::F(n),
                    _ => return None,
                }
            }
            _ => {
                // Single printable char: use it as written (`key_part`,
                // not the lowercased `key`), since terminals deliver a
                // shifted letter as its uppercase char. An explicit
                // `shift+` therefore uppercases the char and is folded
                // into it — `alt+shift+m` and `alt+M` are the same combo.
                let mut chars = key_part[0].chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        let c = if modifiers.contains(KeyModifiers::SHIFT) {
                            c.to_ascii_uppercase()
                        } else {
                            c
                        };
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
        // SHIFT is implicit in the char itself for printable keys under a
        // plain terminal, which delivers the already-shifted char. Under the
        // kitty keyboard protocol, though, crossterm can report the base
        // char (e.g. `Char('z')`) with SHIFT still set alongside CONTROL —
        // stripping SHIFT there would collapse ctrl+shift+z into ctrl+z. So
        // uppercase the char first (a no-op if it's already the shifted
        // form) and then strip SHIFT either way.
        // Also strip SHIFT from BackTab since terminals deliver it with SHIFT still set.
        let (code, mods) = match ev.code {
            KeyCode::Char(c) if ev.modifiers.contains(KeyModifiers::SHIFT) => (
                KeyCode::Char(c.to_ascii_uppercase()),
                ev.modifiers.difference(KeyModifiers::SHIFT),
            ),
            KeyCode::Char(_) | KeyCode::BackTab => {
                (ev.code, ev.modifiers.difference(KeyModifiers::SHIFT))
            }
            _ => (ev.code, ev.modifiers),
        };
        Self {
            code,
            modifiers: mods,
        }
    }
}

pub struct Keymap {
    bindings: HashMap<KeyCombo, Action>,
}

/// Every action a key combo can be bound to by name (`apply_overrides`'
/// left-hand side), plus the parity test's enumeration target: `pub(crate)`
/// so `app::tests`'s mouse-parity sweep (spec §5) can walk it too.
pub(crate) fn named_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("quit", Action::Quit),
        ("focus_next", Action::FocusNext),
        ("focus_prev", Action::FocusPrev),
        ("open_palette", Action::OpenPalette),
        ("close", Action::Close),
        ("editor_tab_1", Action::EditorTabSelect(0)),
        ("editor_tab_2", Action::EditorTabSelect(1)),
        ("editor_tab_3", Action::EditorTabSelect(2)),
        ("editor_tab_4", Action::EditorTabSelect(3)),
        // 2 == EditorTab::Vars.index(); kept a bare literal like the rows
        // above rather than pulling in the editor module here.
        ("tab_vars", Action::EditorTabSelect(2)),
        ("editor_tab_next", Action::EditorTabCycle(1)),
        ("editor_tab_prev", Action::EditorTabCycle(-1)),
        ("cycle_method", Action::CycleMethod),
        ("method_choose", Action::OpenMethodDropdown),
        ("focus_url", Action::FocusUrl),
        ("format_body", Action::FormatBody),
        ("minify_body", Action::MinifyBody),
        ("body_clear", Action::BodyClear),
        ("toggle_body_vars", Action::ToggleBodyVars),
        ("open_body_editor", Action::OpenBodyInEditor),
        ("save", Action::SaveRequest),
        ("send", Action::Send),
        ("project_choose", Action::OpenProjectChooser),
        ("project_cycle", Action::CycleProject),
        ("project_new", Action::PromptNewProject),
        ("env_choose", Action::OpenEnvChooser),
        ("env_cycle", Action::CycleEnv),
        ("pick_variable", Action::OpenVarPicker { completing: false }),
        ("table_add_row", Action::TableAddRow),
        ("toggle_table_collapse", Action::ToggleTableCollapse),
        ("toggle_response_collapse", Action::ToggleResponseCollapse),
        ("var_manager_open", Action::OpenVarManager),
        ("extract_to_variable", Action::ExtractToVariable),
        ("request_duplicate", Action::DuplicateRequest),
        ("undo", Action::Undo),
        ("redo", Action::Redo),
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
            ("alt+1", Action::EditorTabSelect(0)),
            ("alt+2", Action::EditorTabSelect(1)),
            ("alt+3", Action::EditorTabSelect(2)),
            ("alt+4", Action::EditorTabSelect(3)),
            ("alt+right", Action::EditorTabCycle(1)),
            ("alt+left", Action::EditorTabCycle(-1)),
            ("alt+m", Action::CycleMethod),
            ("alt+shift+m", Action::OpenMethodDropdown),
            ("alt+u", Action::FocusUrl),
            ("alt+f", Action::FormatBody),
            ("alt+g", Action::MinifyBody),
            ("alt+x", Action::BodyClear),
            ("alt+b", Action::ToggleBodyVars),
            ("ctrl+e", Action::OpenBodyInEditor),
            ("ctrl+s", Action::SaveRequest),
            ("ctrl+r", Action::Send),
            ("ctrl+enter", Action::Send),
            ("shift+enter", Action::Send),
            ("ctrl+o", Action::OpenProjectChooser),
            ("alt+o", Action::CycleProject),
            ("alt+n", Action::PromptNewProject),
            ("alt+e", Action::OpenEnvChooser),
            ("alt+c", Action::CycleEnv),
            ("ctrl+v", Action::OpenVarPicker { completing: false }),
            ("alt+a", Action::TableAddRow),
            ("alt+p", Action::ToggleTableCollapse),
            ("alt+v", Action::OpenVarManager),
            ("ctrl+shift+e", Action::ExtractToVariable),
            ("ctrl+shift+d", Action::DuplicateRequest),
            ("ctrl+z", Action::Undo),
            ("ctrl+shift+z", Action::Redo),
        ];
        let mut map = Self {
            bindings: HashMap::new(),
        };
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
                .map(|s| KeyCombo::parse(s).ok_or_else(|| anyhow::anyhow!("bad key combo: {s}")))
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

    /// Reverse lookup for the palette's keybinding column: the first combo
    /// (in `format_combo`-sorted order, for a deterministic pick when more
    /// than one is bound — e.g. `quit`'s default `q`/`ctrl+c`) bound to
    /// the action named `action_id` in [`named_actions`], formatted the
    /// way the footer renders combos (caret ctrl, `+`-joined, e.g.
    /// `"^P"`, `"alt+shift+m"`). `None` when `action_id` isn't a known
    /// action name or nothing in this keymap is bound to it — most palette
    /// commands have no keybinding at all, which is an expected, silent
    /// outcome here, not an error.
    pub fn combo_for(&self, action_id: &str) -> Option<String> {
        let target = named_actions()
            .into_iter()
            .find(|(name, _)| *name == action_id)
            .map(|(_, action)| action)?;
        self.bindings
            .iter()
            .filter(|(_, action)| **action == target)
            .map(|(combo, _)| format_combo(combo))
            .min()
    }
}

/// Formats a `KeyCombo` the way the footer displays a combo: `ctrl` as
/// compact caret notation glued to the key (`^P`, `^Enter` — uppercase
/// for single letters), remaining modifiers lowercase and `+`-joined
/// (`alt`, `shift`, in that order) before it. The display inverse of the
/// modifier-folding half of [`KeyCombo::parse`] — a shifted letter
/// (stored as its uppercase `char` with `SHIFT` already folded away)
/// prints back out as `shift+<lowercase>`, and `BackTab` (parsed from
/// `shift+tab`) prints as `shift+tab`.
fn format_combo(combo: &KeyCombo) -> String {
    let mut parts = Vec::new();
    if combo.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
    }
    let (implicit_shift, key) = match combo.code {
        KeyCode::Char(c) if c.is_ascii_uppercase() => (true, c.to_ascii_lowercase().to_string()),
        KeyCode::Char(c) => (false, c.to_string()),
        KeyCode::BackTab => (true, "tab".to_string()),
        KeyCode::Esc => (false, "esc".to_string()),
        KeyCode::Enter => (false, "enter".to_string()),
        KeyCode::Tab => (false, "tab".to_string()),
        KeyCode::Backspace => (false, "backspace".to_string()),
        KeyCode::Up => (false, "up".to_string()),
        KeyCode::Down => (false, "down".to_string()),
        KeyCode::Left => (false, "left".to_string()),
        KeyCode::Right => (false, "right".to_string()),
        KeyCode::F(n) => (false, format!("f{n}")),
        other => (false, format!("{other:?}").to_lowercase()),
    };
    if implicit_shift || combo.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift".to_string());
    }
    let key = if combo.modifiers.contains(KeyModifiers::CONTROL) {
        // Caret notation: uppercase a single letter (`^P`), capitalize a
        // named key (`^Enter`) so the caret always reads as one token.
        let mut chars = key.chars();
        let capped = match chars.next() {
            Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
            None => key,
        };
        format!("^{capped}")
    } else {
        key
    };
    parts.push(key);
    parts.join("+")
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
    fn parses_alt_combo_with_arrow_keys() {
        let c = KeyCombo::parse("alt+left").unwrap();
        assert_eq!(c.code, KeyCode::Left);
        assert_eq!(c.modifiers, KeyModifiers::ALT);
        let c = KeyCombo::parse("alt+right").unwrap();
        assert_eq!(c.code, KeyCode::Right);
        assert_eq!(c.modifiers, KeyModifiers::ALT);
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
        assert_eq!(get("alt+1"), Some(Action::EditorTabSelect(0)));
        assert_eq!(get("alt+2"), Some(Action::EditorTabSelect(1)));
        assert_eq!(get("alt+3"), Some(Action::EditorTabSelect(2)));
        assert_eq!(get("alt+4"), Some(Action::EditorTabSelect(3)));
        assert_eq!(get("alt+right"), Some(Action::EditorTabCycle(1)));
        assert_eq!(get("alt+left"), Some(Action::EditorTabCycle(-1)));
        assert_eq!(get("alt+m"), Some(Action::CycleMethod));
        assert_eq!(get("alt+shift+m"), Some(Action::OpenMethodDropdown));
        assert_eq!(get("alt+u"), Some(Action::FocusUrl));
        assert_eq!(get("alt+f"), Some(Action::FormatBody));
        assert_eq!(get("alt+g"), Some(Action::MinifyBody));
        assert_eq!(get("alt+x"), Some(Action::BodyClear));
        assert_eq!(get("alt+b"), Some(Action::ToggleBodyVars));
        assert_eq!(get("ctrl+e"), Some(Action::OpenBodyInEditor));
        assert_eq!(get("ctrl+r"), Some(Action::Send));
        assert_eq!(get("ctrl+enter"), Some(Action::Send));
        assert_eq!(get("shift+enter"), Some(Action::Send));
        assert_eq!(get("ctrl+o"), Some(Action::OpenProjectChooser));
        assert_eq!(get("alt+o"), Some(Action::CycleProject));
        assert_eq!(get("alt+n"), Some(Action::PromptNewProject));
        assert_eq!(get("alt+e"), Some(Action::OpenEnvChooser));
        assert_eq!(get("alt+c"), Some(Action::CycleEnv));
        assert_eq!(
            get("ctrl+v"),
            Some(Action::OpenVarPicker { completing: false })
        );
        assert_eq!(get("alt+v"), Some(Action::OpenVarManager));
        assert_eq!(get("ctrl+z"), Some(Action::Undo));
        assert_eq!(get("ctrl+shift+z"), Some(Action::Redo));
        assert_eq!(get("ctrl+y"), None, "ctrl+y is deliberately unbound");
    }

    #[test]
    fn ctrl_enter_parses_as_enter_with_control_modifier() {
        let c = KeyCombo::parse("ctrl+enter").unwrap();
        assert_eq!(c.code, KeyCode::Enter);
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn body_actions_are_rebindable_by_name() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(
            r#"
            format_body = "ctrl+j"
            minify_body = "ctrl+k"
            open_body_editor = "f4"
            "#,
        )
        .unwrap();
        let get = |s: &str| m.lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get("ctrl+j"), Some(Action::FormatBody));
        assert_eq!(get("ctrl+k"), Some(Action::MinifyBody));
        assert_eq!(get("f4"), Some(Action::OpenBodyInEditor));
        assert_eq!(get("alt+f"), None, "rebind clears the default combo");
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
    fn method_choose_is_rebindable() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"method_choose = "ctrl+g""#).unwrap();
        assert_eq!(
            m.lookup(&KeyCombo::parse("ctrl+g").unwrap()),
            Some(Action::OpenMethodDropdown)
        );
    }

    #[test]
    fn ctrl_c_is_always_quit_and_cannot_be_taken() {
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"quit = "ctrl+q""#).unwrap();
        assert_eq!(
            m.lookup(&KeyCombo::parse("ctrl+c").unwrap()),
            Some(Action::Quit),
            "ctrl+c survives a quit rebind"
        );
        assert!(
            m.apply_overrides(r#"open_palette = "ctrl+c""#).is_err(),
            "ctrl+c cannot be bound to another action"
        );
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
        assert_eq!(
            get("ctrl+p"),
            Some(Action::OpenPalette),
            "default palette binding intact"
        );
        assert_eq!(
            get("ctrl+q"),
            None,
            "earlier valid rebind must not have been applied"
        );
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
    fn from_event_uppercases_before_stripping_shift_for_kitty_protocol() {
        // Under DISAMBIGUATE_ESCAPE_CODES, crossterm can report the base
        // char with SHIFT|CONTROL rather than the pre-shifted char.
        let ev = KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(
            KeyCombo::from_event(&ev),
            KeyCombo::parse("ctrl+shift+z").unwrap()
        );
    }

    #[test]
    fn combo_for_reverse_looks_up_a_bound_combo() {
        let m = Keymap::default_bindings();
        assert_eq!(m.combo_for("open_palette"), Some("^P".to_string()));
        assert_eq!(m.combo_for("undo"), Some("^Z".to_string()));
    }

    #[test]
    fn combo_for_returns_none_for_unknown_or_unbound_actions() {
        let m = Keymap::default_bindings();
        assert_eq!(m.combo_for("not_a_real_action"), None);
    }

    #[test]
    fn combo_for_finds_a_freshly_bound_combo() {
        let mut m = Keymap::default_bindings();
        m.bind(KeyCombo::parse("f9").unwrap(), Action::OpenVarManager);
        // f9 is a second combo alongside the default alt+v; both resolve.
        let combo = m.combo_for("var_manager_open").unwrap();
        assert!(combo == "f9" || combo == "alt+v", "got {combo:?}");
    }

    #[test]
    fn combo_for_picks_the_lexicographically_first_combo_when_several_are_bound() {
        // quit's defaults are q and ctrl+c — rendered "q" and "^C", and
        // '^' sorts before 'q', so "^C" is the deterministic pick.
        let m = Keymap::default_bindings();
        assert_eq!(m.combo_for("quit"), Some("^C".to_string()));
    }

    #[test]
    fn format_combo_renders_shifted_and_named_keys() {
        assert_eq!(format_combo(&KeyCombo::parse("ctrl+p").unwrap()), "^P");
        assert_eq!(
            format_combo(&KeyCombo::parse("ctrl+shift+z").unwrap()),
            "shift+^Z"
        );
        assert_eq!(
            format_combo(&KeyCombo::parse("ctrl+enter").unwrap()),
            "^Enter"
        );
        assert_eq!(
            format_combo(&KeyCombo::parse("alt+shift+m").unwrap()),
            "alt+shift+m"
        );
        assert_eq!(
            format_combo(&KeyCombo::parse("shift+tab").unwrap()),
            "shift+tab"
        );
        assert_eq!(format_combo(&KeyCombo::parse("f4").unwrap()), "f4");
        assert_eq!(format_combo(&KeyCombo::parse("esc").unwrap()), "esc");
    }

    #[test]
    fn from_event_strips_shift_on_backtab() {
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let c = KeyCombo::from_event(&ev);
        assert_eq!(c.code, KeyCode::BackTab);
        assert_eq!(c.modifiers, KeyModifiers::NONE);
    }
}
