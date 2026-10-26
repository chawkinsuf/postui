use crate::action::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags};
use std::borrow::Cow;
use std::collections::HashMap;

/// The kitty-keyboard-protocol enhancement tier the app pushes (`CSI > 5 u`):
/// DISAMBIGUATE distinguishes chords like ctrl+shift+z from ctrl+z;
/// REPORT_ALTERNATE_KEYS was added to match what Claude Code's TUI pushes,
/// measured empirically — on macOS Ghostty it receives opt/cmd keys with no
/// terminal config while a DISAMBIGUATE-only push did not, and this flag is
/// the only protocol-level difference between the two apps. Behavior-neutral
/// for existing bindings: crossterm folds the alternate (shifted) codepoint
/// back into the pre-shifted char and clears SHIFT, the same shape
/// [`KeyCombo::from_event`] already normalizes.
pub fn app_enhancement_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
}

/// The modifier name keycaps and hints show for ALT: macs label the key
/// "option", so a macOS build advertises `opt+b` while the combo strings
/// themselves stay spelled `alt+b` everywhere else (`KeyCombo::parse`,
/// keys.toml overrides, chip data tuples, tests' key synthesis).
pub fn alt_label() -> &'static str {
    if cfg!(target_os = "macos") { "opt" } else { "alt" }
}

/// The keys that open a *selected* field into an editable one, on every
/// surface with the two-state field model (spec 2026-09-16): Enter and
/// Space. Vim's `i` is a router row in the vim profile (piece 4 spec §4.5).
pub(crate) fn opens_field(ev: &KeyEvent) -> bool {
    matches!(ev.code, KeyCode::Enter | KeyCode::Char(' '))
}

/// Rewrites a display label's leading `alt+` to the platform spelling
/// (`opt+` on macOS — same width, so precomputed keycap layouts hold).
/// Prefix-only on purpose: keycap labels are app-authored (`alt+shift+r`),
/// and the other labels that share the chip painter (methods, statuses,
/// counts) never start with `alt+`.
pub fn display_keycap(label: &str) -> Cow<'_, str> {
    relabel_alt(label, alt_label())
}

fn relabel_alt<'a>(label: &'a str, alt: &str) -> Cow<'a, str> {
    match label.strip_prefix("alt+") {
        Some(rest) if alt != "alt" => Cow::Owned(format!("{alt}+{rest}")),
        _ => Cow::Borrowed(label),
    }
}

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
        Self { code, modifiers: mods }
    }
}

/// Folds SUPER combos — which only kitty-protocol terminals report, and
/// only for keys the terminal itself leaves unbound — into the key each
/// means on macOS. Arrows: cmd+left/right are Home/End, cmd+up/down are
/// ctrl+Home/ctrl+End (buffer start/end), SHIFT riding along so
/// cmd+shift+arrow selects to the same target. Letters: the OS-expected
/// cmd shortcuts become their ctrl spellings (cmd+a select-all as
/// ctrl+shift+a, cmd+z/cmd+shift+z undo/redo, cmd+s save, cmd+v paste) so
/// cmd and ctrl are aliases wherever the terminal delivers cmd at all.
/// cmd+c is deliberately NOT folded — selectionless ctrl+c means quit,
/// and a reflexive mac cmd+c must never quit; `App::handle_key` gives
/// SUPER+c its own copy-only intercept instead. Done once at the top of
/// `App::handle_key` so every component sees the spelling it already
/// implements.
pub fn normalize_super_keys(ev: KeyEvent) -> KeyEvent {
    if !ev.modifiers.contains(KeyModifiers::SUPER) {
        return ev;
    }
    let rest = ev.modifiers - KeyModifiers::SUPER;
    let (code, extra) = match ev.code {
        KeyCode::Left => (KeyCode::Home, KeyModifiers::NONE),
        KeyCode::Right => (KeyCode::End, KeyModifiers::NONE),
        KeyCode::Up => (KeyCode::Home, KeyModifiers::CONTROL),
        KeyCode::Down => (KeyCode::End, KeyModifiers::CONTROL),
        // cmd+a means select-all, whose ctrl spelling is ctrl+shift+a
        // (ctrl+a is line start) — the uppercase char is the shape
        // `KeyCombo::from_event` normalizes shifted letters to.
        KeyCode::Char('a') => (KeyCode::Char('A'), KeyModifiers::CONTROL),
        KeyCode::Char('z' | 'Z' | 's' | 'v') => (ev.code, KeyModifiers::CONTROL),
        _ => return ev,
    };
    KeyEvent::new(code, rest | extra)
}

/// Folds the emacs/readline caret bytes into the GUI keys they mean —
/// ctrl+e becomes End, and ESC b/f (alt+b/f — what macOS terminals send
/// for option+left/right) become ctrl+←/→ word hops, with shift (as a
/// flag or the pre-shifted letter) riding along on the word hops to
/// select. Those apply on every platform: the body editor has always had
/// them via edtui's built-in emacs keymap, so the single-line inputs
/// match it rather than diverge. Only ^A is platform-split (`macos` is
/// `cfg!(target_os = "macos")`, parameterized for tests): on macOS it is
/// the byte cmd+left sends, so it becomes Home; on Linux it passes
/// through and keeps its select-all meaning. ctrl+shift+a (select-all's
/// chord, in either reported spelling) passes through unfolded
/// everywhere. Applied by each text surface (`LineInput::handle_key`,
/// the body editor) ahead of its own key match, so the match only ever
/// sees the GUI spellings.
pub fn fold_text_nav_bytes(ev: KeyEvent, macos: bool) -> KeyEvent {
    match ev.code {
        KeyCode::Char('a') if macos && ev.modifiers == KeyModifiers::CONTROL => {
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)
        }
        KeyCode::Char('e') if ev.modifiers == KeyModifiers::CONTROL => {
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE)
        }
        KeyCode::Char(c @ ('b' | 'B' | 'f' | 'F')) if ev.modifiers.contains(KeyModifiers::ALT) => {
            let arrow = if c.eq_ignore_ascii_case(&'b') { KeyCode::Left } else { KeyCode::Right };
            let mut mods = KeyModifiers::CONTROL;
            if c.is_ascii_uppercase() || ev.modifiers.contains(KeyModifiers::SHIFT) {
                mods |= KeyModifiers::SHIFT;
            }
            KeyEvent::new(arrow, mods)
        }
        _ => ev,
    }
}

/// Where the keyboard is, as far as a key's binding is concerned
/// (`App::key_context`). [`Self::action`] is the one place a key's action
/// takes context: every key press reads its binding through it
/// (`App::lookup_key`), and the palettes' key column checks a row's key
/// against it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyContext {
    /// The sidebar has focus on the main screen.
    pub sidebar_on_main: bool,
}

impl KeyContext {
    /// What a key bound to `action` does here. With the sidebar focused on
    /// the main screen, the split keys (`alt+w`, `shift+alt+w`, or whatever
    /// keys.toml binds to `cycle_split` / `cycle_split_back`) step the
    /// sidebar's width.
    ///
    /// The context belongs to the key, not to the split action (spec S6,
    /// S6a): `:split` and the split chips dispatch their action directly,
    /// never through here, so they always mean the split. The step is
    /// `StepSidebar`, the palette's sidebar-only action (S6b), not vim's
    /// `PaneResize`, which follows the current window.
    pub fn action(self, action: Action) -> Action {
        let step = crate::layout::SIDEBAR_STEP_COLS;
        match action {
            Action::CycleSplit if self.sidebar_on_main => Action::StepSidebar(step),
            Action::CycleSplitBack if self.sidebar_on_main => Action::StepSidebar(-step),
            _ => action,
        }
    }
}

/// `PartialEq` compares the bindings: a live reload uses it to tell a
/// keys.toml that changed from one that was merely re-read, and only
/// re-issues its advisories for the former.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: HashMap<KeyCombo, Action>,
    /// The `keys.toml` rebindings applied on top of the profile's
    /// defaults, in order, so [`Self::rebased`] can lay them on another
    /// profile when the file itself will not parse any more.
    overrides: Vec<(Action, Vec<KeyCombo>)>,
}

impl PartialEq for Keymap {
    fn eq(&self, other: &Self) -> bool {
        self.bindings == other.bindings
    }
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
        // The one tab chord every pane shares: resolved against the
        // focused pane (editor strip, response views, Manage strip) in
        // `App::update`.
        ("tab_next", Action::CycleTabs(1)),
        ("tab_prev", Action::CycleTabs(-1)),
        ("cycle_method", Action::CycleMethod),
        ("method_choose", Action::OpenMethodDropdown),
        ("focus_url", Action::FocusUrl),
        ("format_body", Action::FormatBody),
        ("minify_body", Action::MinifyBody),
        ("body_clear", Action::BodyClear),
        ("toggle_body_vars", Action::ToggleBodyVars),
        ("toggle_insecure", Action::ToggleInsecure),
        ("theme_choose", Action::OpenThemeChooser),
        ("copy_url", Action::CopyToClipboard(crate::action::CopyTarget::Url)),
        ("open_body_editor", Action::OpenBodyInEditor),
        ("save", Action::SaveRequest),
        ("reload", Action::ReloadFromDisk),
        ("discard", Action::DiscardChanges),
        ("send", Action::Send),
        ("project_choose", Action::OpenProjectChooser),
        ("project_next", Action::CycleProject(1)),
        ("project_prev", Action::CycleProject(-1)),
        ("project_new", Action::PromptNewProject),
        ("env_choose", Action::OpenEnvChooser),
        ("env_next", Action::CycleEnv(1)),
        ("env_prev", Action::CycleEnv(-1)),
        ("space_choose", Action::OpenSpaceChooser),
        ("space_next", Action::CycleSpace(1)),
        ("space_prev", Action::CycleSpace(-1)),
        ("space_1", Action::JumpSpace(1)),
        ("space_2", Action::JumpSpace(2)),
        ("space_3", Action::JumpSpace(3)),
        ("space_4", Action::JumpSpace(4)),
        ("space_5", Action::JumpSpace(5)),
        ("space_6", Action::JumpSpace(6)),
        ("space_7", Action::JumpSpace(7)),
        ("space_8", Action::JumpSpace(8)),
        ("space_9", Action::JumpSpace(9)),
        ("pick_variable", Action::OpenVarPicker { completing: false }),
        ("paste", Action::Paste),
        ("table_add_row", Action::TableAddRow),
        ("toggle_table_collapse", Action::ToggleTableCollapse),
        ("toggle_response_collapse", Action::ToggleResponseCollapse),
        ("cycle_split", Action::CycleSplit),
        ("cycle_split_back", Action::CycleSplitBack),
        ("manage_open", Action::OpenManage { tab: None }),
        ("extract_to_variable", Action::ExtractToVariable),
        ("request_duplicate", Action::DuplicateRequest),
        ("undo", Action::Undo),
        ("redo", Action::Redo),
        ("toggle_jq_bar", Action::ToggleJqBar),
        ("open_jq_bar", Action::OpenJqBar),
        ("open_jq_views", Action::OpenJqViews),
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
    /// The bindings `profile` ships with (piece 4 spec §4.4, plan M1
    /// Deviation 4). Vim gives up `q` (its macro prefix; `:q` quits),
    /// `ctrl+r` (redo, a router row), `alt+a` (`a` adds a table row, plan
    /// M2 Deviation 5), `alt+←/→` (`gT`/`gt`, plan M4a), and `ctrl+s`,
    /// `alt+w` and `shift+alt+w` (`:w`, `:split`, `:splitback`, plan M4b).
    /// A user's `keys.toml` layers on top.
    pub fn default_bindings_for(profile: crate::config::KeyProfile) -> Self {
        let mut map = Self::default_bindings();
        if profile == crate::config::KeyProfile::Vim {
            for combo in
                ["q", "ctrl+r", "alt+a", "alt+left", "alt+right", "ctrl+s", "alt+w", "shift+alt+w"]
            {
                map.bindings.remove(&KeyCombo::parse(combo).expect("a valid default combo"));
            }
        }
        map
    }

    pub fn default_bindings() -> Self {
        let defaults = [
            ("q", Action::Quit),
            ("ctrl+c", Action::Quit),
            ("tab", Action::FocusNext),
            ("shift+tab", Action::FocusPrev),
            ("ctrl+p", Action::OpenPalette),
            ("esc", Action::Close),
            // Spaces own the ctrl-digits (i3 style: the number swaps the
            // whole working set). ctrl, not alt: Ghostty on Linux takes
            // alt+1..9 for its own tabs. ctrl+digit has no legacy byte, so
            // it needs the kitty protocol the app pushes — the same
            // dependency as every other ctrl chord here. Editor tabs keep
            // alt+left/right and clicks; `editor_tab_N` stays bindable
            // from keys.toml.
            ("ctrl+1", Action::JumpSpace(1)),
            ("ctrl+2", Action::JumpSpace(2)),
            ("ctrl+3", Action::JumpSpace(3)),
            ("ctrl+4", Action::JumpSpace(4)),
            ("ctrl+5", Action::JumpSpace(5)),
            ("ctrl+6", Action::JumpSpace(6)),
            ("ctrl+7", Action::JumpSpace(7)),
            ("ctrl+8", Action::JumpSpace(8)),
            ("ctrl+9", Action::JumpSpace(9)),
            // The header's three selectors sit on the bottom row in
            // their on-screen order — project, environment, space on
            // alt+z / alt+x / alt+c — so the keys are positional rather
            // than mnemonic. A letter with shift for reverse, like the
            // split's alt+w: the bracket chords (alt+] / alt+[) never
            // arrived through opt on macOS. The env and space dropdowns
            // open by click or from the palette; the project chooser
            // keeps ctrl+o.
            ("alt+z", Action::CycleProject(1)),
            ("alt+shift+z", Action::CycleProject(-1)),
            ("alt+x", Action::CycleEnv(1)),
            ("alt+shift+x", Action::CycleEnv(-1)),
            ("alt+c", Action::CycleSpace(1)),
            ("alt+shift+c", Action::CycleSpace(-1)),
            ("alt+right", Action::CycleTabs(1)),
            ("alt+left", Action::CycleTabs(-1)),
            ("alt+m", Action::CycleMethod),
            ("alt+shift+m", Action::OpenMethodDropdown),
            ("alt+u", Action::FocusUrl),
            // alt+f and alt+b ship unbound: they're the ESC f/b bytes mac
            // terminals send for option+arrow word motion (see
            // `fold_text_nav_bytes`), so format-body and the theme
            // chooser live on alt+j and alt+t. Same for ctrl+a/ctrl+e
            // (^A/^E — and ctrl+a is select-all on Linux). A user's
            // keys.toml may still bind any of them deliberately.
            ("alt+j", Action::FormatBody),
            ("alt+g", Action::MinifyBody),
            ("alt+k", Action::BodyClear),
            ("alt+s", Action::ToggleBodyVars),
            ("alt+i", Action::ToggleInsecure),
            ("alt+t", Action::OpenThemeChooser),
            ("alt+y", Action::CopyToClipboard(crate::action::CopyTarget::Url)),
            ("ctrl+s", Action::SaveRequest),
            ("alt+shift+r", Action::ReloadFromDisk),
            ("alt+d", Action::DiscardChanges),
            ("ctrl+r", Action::Send),
            ("ctrl+enter", Action::Send),
            ("shift+enter", Action::Send),
            ("ctrl+o", Action::OpenProjectChooser),
            ("alt+e", Action::OpenBodyInEditor),
            ("ctrl+v", Action::Paste),
            ("alt+v", Action::OpenVarPicker { completing: false }),
            ("alt+a", Action::TableAddRow),
            ("alt+p", Action::ToggleTableCollapse),
            // Hiding the response pairs with hiding the editor the way
            // shift+alt+w pairs with alt+w (spec E12b).
            ("shift+alt+p", Action::ToggleResponseCollapse),
            ("alt+w", Action::CycleSplit),
            ("shift+alt+w", Action::CycleSplitBack),
            ("alt+r", Action::OpenManage { tab: None }),
            ("ctrl+shift+e", Action::ExtractToVariable),
            ("ctrl+shift+d", Action::DuplicateRequest),
            ("ctrl+z", Action::Undo),
            ("ctrl+shift+z", Action::Redo),
            ("alt+q", Action::OpenJqBar),
            ("alt+shift+q", Action::ToggleJqBar),
            ("alt+shift+j", Action::OpenJqViews),
        ];
        let mut map = Self { bindings: HashMap::new(), overrides: Vec::new() };
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

    /// Whether `combo`'s binding is one a `keys.toml` override put there,
    /// never a default binding (plan M2 Deviation 18: in arrows only the
    /// user's letters beat the router's verb rows).
    pub(crate) fn is_override(&self, combo: &KeyCombo) -> bool {
        self.bindings.get(combo).is_some_and(|action| {
            self.overrides.iter().any(|(a, combos)| a == action && combos.contains(combo))
        })
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
        self.rebind(&resolved);
        self.overrides.extend(resolved);
        Ok(())
    }

    /// Lays validated rebindings on the current bindings.
    fn rebind(&mut self, overrides: &[(Action, Vec<KeyCombo>)]) {
        for (action, combos) in overrides {
            self.bindings.retain(|_, a| a != action); // rebind removes old combo(s)
            for combo in combos {
                self.bind(combo.clone(), action.clone());
            }
        }
        // ctrl+c is always reserved for quit, regardless of overrides.
        self.bind(KeyCombo::parse("ctrl+c").unwrap(), Action::Quit);
    }

    /// `profile`'s defaults with this keymap's `keys.toml` rebindings on
    /// top: the keys the user is typing with, on another profile. A
    /// profile switch uses it when `keys.toml` will not parse, so the
    /// switch keeps the live bindings, as a reload does, rather than
    /// dropping them or keeping the old profile's defaults.
    pub fn rebased(&self, profile: crate::config::KeyProfile) -> Self {
        let mut map = Self::default_bindings_for(profile);
        map.rebind(&self.overrides);
        map.overrides = self.overrides.clone();
        map
    }

    /// The defaults with `contents` (a `keys.toml`) applied, or the error
    /// text when the file will not apply.
    ///
    /// `apply_overrides` validates the whole file before binding anything,
    /// so one bad entry leaves every default in place — a fact the user
    /// has to hear, or every rebinding just silently stops working. What
    /// "in place" then *means* is the caller's call, not this function's:
    /// startup falls back to the defaults, a live reload keeps the keys
    /// the user is already typing with. So the error comes back bare and
    /// each caller words the consequence itself.
    pub fn try_from_overrides(contents: &str) -> Result<Self, String> {
        Self::try_from_overrides_for(crate::config::KeyProfile::Arrows, contents)
    }

    /// [`Self::try_from_overrides`] on `profile`'s own defaults: the user's
    /// `keys.toml` layers on whichever profile is active, and wins.
    pub fn try_from_overrides_for(
        profile: crate::config::KeyProfile,
        contents: &str,
    ) -> Result<Self, String> {
        let mut map = Self::default_bindings_for(profile);
        map.apply_overrides(contents).map_err(|e| e.to_string())?;
        Ok(map)
    }

    /// The caret-conflict warnings for the startup toasts: on a macOS
    /// build (`macos` — parameterized for tests), a keys.toml override on
    /// one of the caret-motion bytes (^A/^E = cmd+left/right, ESC b/f =
    /// option+arrows) wins per the user's explicit config, but silently
    /// costs the cmd/option+arrow gesture that sends the same byte —
    /// worth a warning, not a rejection (an error would atomically
    /// discard the whole override file, and the same dotfile is legal on
    /// Linux). On Linux the list is always empty: ctrl+a select-all and
    /// friends are component behavior there, and an override shadowing
    /// them is an ordinary, deliberate rebind.
    pub fn caret_warnings(&self, macos: bool) -> Vec<String> {
        if macos { self.caret_conflicts() } else { Vec::new() }
    }

    /// The `keys.toml` bindings of a bare char that some vim row starts
    /// with (piece 4 spec §4.4, plan M1 Deviation 19). The binding wins
    /// while nothing is pending, so the user hears once, at load, which
    /// vim key it hides. A digit `1`–`9` is the other way round: vim reads
    /// it as a count first on every surface a `keys.toml` letter reaches,
    /// so that binding never runs. The vim defaults bind no bare char, so
    /// every one found came from `keys.toml`.
    pub fn vim_shadow_warnings(&self) -> Vec<String> {
        let firsts = crate::keyroute::table::vim_first_chars();
        let mut out: Vec<String> = self
            .bindings
            .keys()
            .filter_map(|c| match c.code {
                KeyCode::Char(ch) if c.modifiers.is_empty() && firsts.contains(&ch) => {
                    Some(format!("keys.toml binds `{ch}`, which shadows vim's `{ch}`"))
                }
                KeyCode::Char(ch @ '1'..='9') if c.modifiers.is_empty() => Some(format!(
                    "keys.toml binds `{ch}`, which vim reads as a count, so the binding never runs"
                )),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// The macOS caret-conflict report for [`Self::caret_warnings`]: one
    /// line per caret-motion combo something is bound to, naming the
    /// gesture the binding shadows.
    fn caret_conflicts(&self) -> Vec<String> {
        [
            ("ctrl+a", "cmd+left / line start"),
            ("ctrl+e", "cmd+right / line end"),
            ("alt+b", "option+left word motion"),
            ("alt+f", "option+right word motion"),
        ]
        .iter()
        .filter_map(|(combo, gesture)| {
            // Combos here are compile-time constants; parse cannot fail.
            let parsed = KeyCombo::parse(combo)?;
            self.lookup(&parsed)
                .map(|_| format!("keys.toml binds {combo}, which shadows {gesture} on macOS"))
        })
        .collect()
    }

    /// [`Self::combo_for_action`] by the action's `keys.toml` name: the
    /// first combo (in `format_combo`-sorted order, for a deterministic
    /// pick when more than one is bound — e.g. `quit`'s default
    /// `q`/`ctrl+c`) bound to the action named `action_id` in
    /// [`named_actions`], formatted the way the footer renders combos
    /// (caret ctrl, `+`-joined, e.g. `"^P"`, `"alt+shift+m"`). `None` when
    /// `action_id` isn't a known action name or nothing in this keymap is
    /// bound to it. Tests only: the palette's key column asks by the action
    /// itself (task #105).
    #[cfg(test)]
    pub fn combo_for(&self, action_id: &str) -> Option<String> {
        let target = named_actions()
            .into_iter()
            .find(|(name, _)| *name == action_id)
            .map(|(_, action)| action)?;
        self.combo_for_action(&target)
    }

    /// The combo bound to `target`, formatted the way the footer renders
    /// combos (caret ctrl, `+`-joined, e.g. `"^P"`, `"alt+shift+m"`), the
    /// first in sorted order when more than one is bound; `None` when
    /// nothing in this keymap is. What a keycap and the palette's key
    /// column spell from, every frame.
    pub fn combo_for_action(&self, target: &Action) -> Option<String> {
        let combos: Vec<&KeyCombo> = self
            .bindings
            .iter()
            .filter(|(_, action)| *action == target)
            .map(|(combo, _)| combo)
            .collect();
        // A bare-key alias a `keys.toml` adds (":", "u") has no modifiers,
        // and its formatted form (":", "U") can sort below the chord it's a
        // synonym for ("^P", "^Z") -- e.g. ":" < "^P" -- which would flip
        // the hint to the alias. Prefer a modified combo (a chord) over a
        // bare key first, and only then take the minimum, so the footer
        // hint keeps advertising the chord.
        let modified: Vec<&&KeyCombo> = combos.iter().filter(|c| !c.modifiers.is_empty()).collect();
        let pool: Vec<&KeyCombo> =
            if modified.is_empty() { combos } else { modified.into_iter().copied().collect() };
        pool.into_iter().map(format_combo).min()
    }

    /// Every combo bound to `action_id`, in `keys.toml`'s input grammar
    /// (via [`format_combo_config`], not the footer's `format_combo`
    /// keycap), sorted so a seeded `keys.toml` is byte-stable across runs.
    ///
    /// The plural matters: `apply_overrides` removes *all* of an action's
    /// existing combos before binding the ones it is given, so a seed
    /// listing only one would silently unbind the rest the moment a user
    /// uncommented it -- taking the other defaults with it (`quit`'s `q`
    /// beside its `ctrl+c`).
    pub fn combos_for(&self, action_id: &str) -> Vec<String> {
        let Some((_, action)) = named_actions().into_iter().find(|(n, _)| n == &action_id) else {
            return Vec::new();
        };
        let mut combos: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(combo, _)| format_combo_config(combo))
            .collect();
        combos.sort();
        combos
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
pub(crate) fn format_combo(combo: &KeyCombo) -> String {
    let mut parts = Vec::new();
    if combo.modifiers.contains(KeyModifiers::ALT) {
        parts.push(alt_label().to_string());
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

/// The inverse of [`KeyCombo::parse`]: renders a `KeyCombo` in the exact
/// grammar `keys.toml` accepts as input (`ctrl+p`, `alt+shift+m`, `f9`,
/// `shift+tab`). Deliberately not [`format_combo`], which renders the
/// footer's caret keycap (`^P`) — a display convention `parse` does not
/// understand, and which is spelled with `alt_label()`'s platform text
/// ("opt" on macOS) rather than the literal `alt+` the parser requires.
/// Used wherever a combo is written into a `keys.toml` a user will
/// hand-edit or uncomment (see [`Keymap::combos_for`]), so what lands
/// there is always exactly what `parse` will read back.
fn format_combo_config(combo: &KeyCombo) -> String {
    let mut parts = Vec::new();
    if combo.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl".to_string());
    }
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
    parts.push(key);
    parts.join("+")
}

/// A fully commented `keys.toml` listing every bindable action at its
/// current binding. Commented for the same reason `config_seed` is, and
/// more urgently: `keys.toml` is an overrides file, so an uncommented
/// seed would pin every binding forever and no future default would ever
/// reach the user.
///
/// Each line lists *every* combo bound to its action (see `combos_for`),
/// because `apply_overrides` replaces all of them -- a line naming one
/// would unbind the rest when uncommented.
pub fn keys_seed(map: &Keymap) -> String {
    let mut out = String::from(
        "\
# postui key bindings. Uncomment a line to rebind its action.
# Each line lists every combo currently bound to that action; the list
# replaces them all, so keep the ones you want to keep.
# ctrl+c is reserved for quit and cannot be rebound.

",
    );
    for (name, _) in named_actions() {
        let combos = map.combos_for(name);
        if combos.is_empty() {
            continue;
        }
        let list = combos.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("# {name} = [{list}]\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bad_keys_file_is_reported_and_applies_nothing() {
        let err = super::Keymap::try_from_overrides("save = \"ctrl+s\"\nnope = \"ctrl+\"\n")
            .expect_err("the bad entry is reported");
        assert!(err.contains("unknown action"), "{err}");
        let map = super::Keymap::try_from_overrides("").expect("an empty file is fine");
        let save = super::KeyCombo::parse("ctrl+s").unwrap();
        assert_eq!(
            map.lookup(&save),
            super::Keymap::default_bindings().lookup(&save),
            "nothing from the bad file was applied"
        );
    }

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
    fn relabel_alt_swaps_only_the_keycap_prefix() {
        assert_eq!(relabel_alt("alt+b", "opt"), "opt+b");
        assert_eq!(relabel_alt("alt+shift+v", "opt"), "opt+shift+v");
        // Identity spelling borrows; non-keycap labels pass through.
        assert_eq!(relabel_alt("alt+b", "alt"), "alt+b");
        assert_eq!(relabel_alt("GET", "opt"), "GET");
        assert_eq!(relabel_alt("^P", "opt"), "^P");
    }

    #[test]
    fn super_arrows_normalize_to_home_end() {
        let n = |code, mods| normalize_super_keys(KeyEvent::new(code, mods));
        let ev = n(KeyCode::Left, KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Home, KeyModifiers::NONE));
        let ev = n(KeyCode::Right, KeyModifiers::SUPER | KeyModifiers::SHIFT);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::End, KeyModifiers::SHIFT));
        let ev = n(KeyCode::Up, KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Home, KeyModifiers::CONTROL));
        let ev = n(KeyCode::Down, KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::End, KeyModifiers::CONTROL));
        // Untouched: plain arrows.
        let ev = n(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Left, KeyModifiers::NONE));
    }

    #[test]
    fn fold_text_nav_bytes_splits_only_ctrl_a_by_platform() {
        let f = |code, mods, macos| fold_text_nav_bytes(KeyEvent::new(code, mods), macos);
        // Both platforms: ctrl+e becomes End, ESC b/f become ctrl+arrow
        // hops — matching edtui's built-in emacs keys in the body.
        for macos in [true, false] {
            let ev = f(KeyCode::Char('e'), KeyModifiers::CONTROL, macos);
            assert_eq!((ev.code, ev.modifiers), (KeyCode::End, KeyModifiers::NONE));
            let ev = f(KeyCode::Char('b'), KeyModifiers::ALT, macos);
            assert_eq!((ev.code, ev.modifiers), (KeyCode::Left, KeyModifiers::CONTROL));
            let ev = f(KeyCode::Char('F'), KeyModifiers::ALT, macos);
            assert_eq!(
                (ev.code, ev.modifiers),
                (KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                "the pre-shifted letter selects the hop"
            );
            // Select-all's chord passes through unfolded everywhere.
            let ev = f(KeyCode::Char('a'), KeyModifiers::CONTROL | KeyModifiers::SHIFT, macos);
            assert_eq!(
                (ev.code, ev.modifiers),
                (KeyCode::Char('a'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
            );
        }
        // Only ^A splits: it's cmd+left's byte on macOS, select-all's key
        // on Linux.
        let ev = f(KeyCode::Char('a'), KeyModifiers::CONTROL, true);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Home, KeyModifiers::NONE));
        let ev = f(KeyCode::Char('a'), KeyModifiers::CONTROL, false);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('a'), KeyModifiers::CONTROL));
    }

    #[test]
    fn super_letters_fold_to_their_ctrl_spellings() {
        let n = |code, mods| normalize_super_keys(KeyEvent::new(code, mods));
        // cmd+a means select-all: the ctrl+shift+a spelling, pre-shifted.
        let ev = n(KeyCode::Char('a'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('A'), KeyModifiers::CONTROL));
        let ev = n(KeyCode::Char('z'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('z'), KeyModifiers::CONTROL));
        // cmd+shift+z redo, in both reported spellings.
        let ev = n(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT);
        assert_eq!(
            (ev.code, ev.modifiers),
            (KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        );
        let ev = n(KeyCode::Char('Z'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('Z'), KeyModifiers::CONTROL));
        let ev = n(KeyCode::Char('s'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('s'), KeyModifiers::CONTROL));
        let ev = n(KeyCode::Char('v'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('v'), KeyModifiers::CONTROL));
        // cmd+c is NOT folded — App::handle_key intercepts it copy-only,
        // and folding to ctrl+c would make a reflexive cmd+c quit.
        let ev = n(KeyCode::Char('c'), KeyModifiers::SUPER);
        assert_eq!((ev.code, ev.modifiers), (KeyCode::Char('c'), KeyModifiers::SUPER));
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
        assert_eq!(
            get(":"),
            None,
            "the alias left the arrows profile (spec §4.5); vim has a router row"
        );
        assert_eq!(get("esc"), Some(Action::Close));
        // ctrl-digits, not alt-digits: Ghostty on Linux owns alt+1..9
        // for its own tabs. The header selectors take the bottom row in
        // on-screen order (z/x/c), shift for reverse — not alt+] / alt+[:
        // the bracket chords never arrived through opt on macOS.
        assert_eq!(get("ctrl+1"), Some(Action::JumpSpace(1)));
        assert_eq!(get("ctrl+4"), Some(Action::JumpSpace(4)));
        assert_eq!(get("ctrl+9"), Some(Action::JumpSpace(9)));
        assert_eq!(get("alt+1"), None);
        assert_eq!(get("alt+z"), Some(Action::CycleProject(1)));
        assert_eq!(get("alt+shift+z"), Some(Action::CycleProject(-1)));
        assert_eq!(get("alt+x"), Some(Action::CycleEnv(1)));
        assert_eq!(get("alt+shift+x"), Some(Action::CycleEnv(-1)));
        assert_eq!(get("alt+c"), Some(Action::CycleSpace(1)));
        assert_eq!(get("alt+shift+c"), Some(Action::CycleSpace(-1)));
        assert_eq!(get("alt+l"), None);
        assert_eq!(get("alt+o"), None);
        assert_eq!(get("alt+]"), None);
        assert_eq!(get("alt+["), None);
        assert_eq!(get("alt+shift+s"), None);
        assert_eq!(get("alt+shift+e"), None);
        assert_eq!(get("alt+right"), Some(Action::CycleTabs(1)));
        assert_eq!(get("alt+left"), Some(Action::CycleTabs(-1)));
        assert_eq!(get("alt+m"), Some(Action::CycleMethod));
        assert_eq!(get("alt+shift+m"), Some(Action::OpenMethodDropdown));
        assert_eq!(get("alt+u"), Some(Action::FocusUrl));
        assert_eq!(get("alt+j"), Some(Action::FormatBody));
        assert_eq!(get("alt+g"), Some(Action::MinifyBody));
        assert_eq!(get("alt+k"), Some(Action::BodyClear));
        assert_eq!(get("alt+s"), Some(Action::ToggleBodyVars));
        assert_eq!(get("alt+i"), Some(Action::ToggleInsecure));
        assert_eq!(get("alt+t"), Some(Action::OpenThemeChooser));
        assert_eq!(get("alt+w"), Some(Action::CycleSplit));
        assert_eq!(get("shift+alt+w"), Some(Action::CycleSplitBack));
        assert_eq!(get("alt+p"), Some(Action::ToggleTableCollapse));
        assert_eq!(get("shift+alt+p"), Some(Action::ToggleResponseCollapse));
        assert_eq!(get("alt+d"), Some(Action::DiscardChanges));
        assert_eq!(get("alt+b"), None, "alt+b is reserved for word-left (ESC b)");
        assert_eq!(get("alt+f"), None, "alt+f is reserved for word-right (ESC f)");
        assert_eq!(get("alt+y"), Some(Action::CopyToClipboard(crate::action::CopyTarget::Url)));
        assert_eq!(get("alt+e"), Some(Action::OpenBodyInEditor));
        assert_eq!(
            get("ctrl+e"),
            None,
            "ctrl+e is free again (edtui's emacs end-of-line gets it in the body)"
        );
        assert_eq!(get("ctrl+r"), Some(Action::Send));
        assert_eq!(
            get("alt+shift+r"),
            Some(Action::ReloadFromDisk),
            "the user-triggered reload lives on a shifted chord; it is rare"
        );
        assert_eq!(get("ctrl+enter"), Some(Action::Send));
        assert_eq!(get("shift+enter"), Some(Action::Send));
        assert_eq!(get("ctrl+o"), Some(Action::OpenProjectChooser));
        assert_eq!(
            get("alt+n"),
            None,
            "new project is reached from the Projects chooser, not a chord"
        );
        assert_eq!(get("ctrl+v"), Some(Action::Paste), "ctrl+v pastes now");
        assert_eq!(
            get("alt+v"),
            Some(Action::OpenVarPicker { completing: false }),
            "the variable picker sits on alt+v; ctrl+v is the OS paste chord"
        );
        assert_eq!(get("alt+r"), Some(Action::OpenManage { tab: None }));
        assert_eq!(get("ctrl+z"), Some(Action::Undo));
        assert_eq!(
            get("u"),
            None,
            "the alias left the arrows profile (spec §4.5); vim has a router row"
        );
        assert_eq!(get("ctrl+shift+z"), Some(Action::Redo));
        assert_eq!(get("ctrl+y"), None, "ctrl+y is deliberately unbound");
        assert_eq!(get("alt+q"), Some(Action::OpenJqBar));
        assert_eq!(get("alt+shift+q"), Some(Action::ToggleJqBar));
    }

    /// jq views: the dropdown has a chord, the same in both profiles, and
    /// a name `keys.toml` can rebind.
    #[test]
    fn alt_shift_j_opens_the_jq_views_in_both_profiles() {
        for profile in [crate::config::KeyProfile::Arrows, crate::config::KeyProfile::Vim] {
            let m = Keymap::default_bindings_for(profile);
            assert_eq!(
                m.lookup(&KeyCombo::parse("alt+shift+j").unwrap()),
                Some(Action::OpenJqViews),
                "{profile:?}"
            );
        }
        assert!(
            named_actions().iter().any(|(n, a)| *n == "open_jq_views" && *a == Action::OpenJqViews)
        );
        assert_eq!(
            Keymap::default_bindings().combo_for("open_jq_views"),
            Some("alt+shift+j".to_string())
        );
    }

    #[test]
    fn editor_tab_numbers_stay_bindable_but_unbound_by_default() {
        let m = Keymap::default_bindings();
        assert!(
            named_actions()
                .iter()
                .any(|(n, a)| *n == "editor_tab_1" && *a == Action::EditorTabSelect(0))
        );
        assert_eq!(m.combo_for("editor_tab_1"), None);
        assert_eq!(m.combo_for("space_1"), Some("^1".to_string()));
        assert_eq!(m.combo_for("space_next"), Some("alt+c".to_string()));
        assert_eq!(m.combo_for("space_prev"), Some("alt+shift+c".to_string()));
        assert_eq!(m.combo_for("env_next"), Some("alt+x".to_string()));
        assert_eq!(m.combo_for("project_prev"), Some("alt+shift+z".to_string()));
    }

    #[test]
    fn bracket_and_digit_combos_parse() {
        assert_eq!(KeyCombo::parse("alt+]").unwrap().code, KeyCode::Char(']'));
        assert_eq!(KeyCombo::parse("alt+[").unwrap().code, KeyCode::Char('['));
        let c = KeyCombo::parse("ctrl+1").unwrap();
        assert_eq!(c.code, KeyCode::Char('1'));
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);
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
        assert_eq!(get("alt+j"), None, "rebind clears the default combo");
    }

    #[test]
    fn caret_motion_keys_ship_unbound_but_stay_user_overridable() {
        // alt+b/f and ctrl+a/e are the caret-motion bytes (ESC b/f,
        // ^A/^E; ctrl+a is select-all on Linux), so no DEFAULT may claim
        // them — but a user's keys.toml may knowingly bind them.
        let m = Keymap::default_bindings();
        for combo in ["alt+b", "alt+f", "ctrl+a", "ctrl+e"] {
            assert_eq!(
                m.lookup(&KeyCombo::parse(combo).unwrap()),
                None,
                "{combo} must ship unbound"
            );
        }
        let mut m = Keymap::default_bindings();
        m.apply_overrides(r#"format_body = "alt+f""#).unwrap();
        assert_eq!(
            m.lookup(&KeyCombo::parse("alt+f").unwrap()),
            Some(Action::FormatBody),
            "an explicit override may still claim a caret-motion key"
        );
    }

    #[test]
    fn caret_conflicts_report_shadowed_mac_gestures() {
        let m = Keymap::default_bindings();
        assert!(m.caret_conflicts().is_empty(), "defaults never claim a caret-motion key");
        let mut m = Keymap::default_bindings();
        m.apply_overrides(
            r#"
            format_body = "alt+f"
            save = "ctrl+a"
            "#,
        )
        .unwrap();
        let warnings = m.caret_conflicts();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("ctrl+a") && w.contains("cmd+left")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("alt+f") && w.contains("option+right")),
            "{warnings:?}"
        );
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
        assert_eq!(m.lookup(&KeyCombo::parse("ctrl+g").unwrap()), Some(Action::OpenMethodDropdown));
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
    fn from_event_uppercases_before_stripping_shift_for_kitty_protocol() {
        // Under DISAMBIGUATE_ESCAPE_CODES, crossterm can report the base
        // char with SHIFT|CONTROL rather than the pre-shifted char.
        let ev = KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(KeyCombo::from_event(&ev), KeyCombo::parse("ctrl+shift+z").unwrap());
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
        m.bind(KeyCombo::parse("f9").unwrap(), Action::OpenManage { tab: None });
        // f9 is a second combo alongside the default alt+r; both resolve.
        let combo = m.combo_for("manage_open").unwrap();
        assert!(combo == "f9" || combo == "alt+r", "got {combo:?}");
    }

    #[test]
    fn combo_for_picks_the_lexicographically_first_combo_when_several_are_bound() {
        // quit's defaults are q and ctrl+c — rendered "q" and "^C", and
        // '^' sorts before 'q', so "^C" is the deterministic pick.
        let m = Keymap::default_bindings();
        assert_eq!(m.combo_for("quit"), Some("^C".to_string()));
    }

    /// Seeding keys.toml from the singular accessor would drop aliases, and
    /// `apply_overrides` replaces *all* of an action's combos -- so
    /// uncommenting a seeded line would silently unbind the others.
    #[test]
    fn combos_for_lists_every_alias_not_just_one() {
        let map = Keymap::default_bindings();
        let mut found_multi = false;
        for (name, _) in named_actions() {
            let combos = map.combos_for(name);
            if let Some(one) = map.combo_for(name) {
                let as_keycaps: Vec<String> = combos
                    .iter()
                    .map(|c| {
                        format_combo(
                            &KeyCombo::parse(c).expect("combos_for must emit parseable combos"),
                        )
                    })
                    .collect();
                assert!(
                    as_keycaps.contains(&one),
                    "{name}: combos_for must include what combo_for returns"
                );
            }
            if combos.len() > 1 {
                found_multi = true;
            }
        }
        assert!(found_multi, "the default map has aliased actions; combos_for must expose them");
    }

    #[test]
    fn combos_for_is_stable_across_calls() {
        let map = Keymap::default_bindings();
        for (name, _) in named_actions() {
            assert_eq!(
                map.combos_for(name),
                map.combos_for(name),
                "{name}: a seed built twice must be identical"
            );
        }
    }

    #[test]
    fn combos_for_an_unknown_action_is_empty() {
        assert!(Keymap::default_bindings().combos_for("no_such_action").is_empty());
    }

    #[test]
    fn format_combo_renders_shifted_and_named_keys() {
        assert_eq!(format_combo(&KeyCombo::parse("ctrl+p").unwrap()), "^P");
        assert_eq!(format_combo(&KeyCombo::parse("ctrl+shift+z").unwrap()), "shift+^Z");
        assert_eq!(format_combo(&KeyCombo::parse("ctrl+enter").unwrap()), "^Enter");
        assert_eq!(format_combo(&KeyCombo::parse("alt+shift+m").unwrap()), "alt+shift+m");
        assert_eq!(format_combo(&KeyCombo::parse("shift+tab").unwrap()), "shift+tab");
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

    /// The load-bearing test for `combos_for`: uncommenting any seeded line
    /// must reproduce exactly the bindings that line names, aliases and all.
    #[test]
    fn uncommenting_any_seeded_line_changes_nothing() {
        let defaults = Keymap::default_bindings();
        let seed = keys_seed(&defaults);
        let live: String = seed
            .lines()
            .filter_map(|l| l.strip_prefix("# "))
            .filter(|l| l.contains(" = ["))
            .collect::<Vec<_>>()
            .join("\n");
        let applied = Keymap::try_from_overrides(&live)
            .expect("an uncommented seed must be a valid overrides file");
        for (name, _) in named_actions() {
            assert_eq!(
                applied.combos_for(name),
                defaults.combos_for(name),
                "{name} changed when its seeded line was uncommented"
            );
        }
    }

    #[test]
    fn the_keys_seed_is_entirely_commented_and_is_a_no_op() {
        let defaults = Keymap::default_bindings();
        let seed = keys_seed(&defaults);
        for line in seed.lines() {
            let line = line.trim();
            assert!(line.is_empty() || line.starts_with('#'), "seed line is live: {line:?}");
        }
        let applied = Keymap::try_from_overrides(&seed).unwrap();
        for (name, _) in named_actions() {
            assert_eq!(applied.combos_for(name), defaults.combos_for(name));
        }
    }

    #[test]
    fn opens_field_matches_enter_and_space_only() {
        let key = |code: KeyCode, mods: KeyModifiers| KeyEvent::new(code, mods);
        assert!(opens_field(&key(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(opens_field(&key(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(!opens_field(&key(KeyCode::Char('i'), KeyModifiers::NONE)));
        assert!(!opens_field(&key(KeyCode::Char('I'), KeyModifiers::SHIFT)));
        assert!(!opens_field(&key(KeyCode::Char('i'), KeyModifiers::CONTROL)));
        assert!(!opens_field(&key(KeyCode::Char('j'), KeyModifiers::NONE)));
        assert!(!opens_field(&key(KeyCode::Esc, KeyModifiers::NONE)));
    }

    /// Plan M1 Deviation 4, M2 Deviation 5 and M4a Deviation 12: vim gives
    /// up `q` (its macro prefix; `:q` quits), `ctrl+r` (redo, a router row),
    /// `alt+a` (`a` adds a row) and `alt+←/→` (`gT`/`gt`). Every other chord
    /// stays shared until the milestone that gives vim its own key for it.
    #[test]
    fn the_vim_profile_drops_q_and_gives_ctrl_r_to_the_router() {
        use crate::config::KeyProfile::{Arrows, Vim};
        let get = |p, s: &str| Keymap::default_bindings_for(p).lookup(&KeyCombo::parse(s).unwrap());
        assert_eq!(get(Arrows, "q"), Some(Action::Quit));
        assert_eq!(get(Arrows, "ctrl+r"), Some(Action::Send));
        assert_eq!(get(Vim, "q"), None);
        assert_eq!(get(Vim, "ctrl+r"), None, "a router row (redo), not a keymap binding");
        assert_eq!(get(Vim, "ctrl+enter"), Some(Action::Send));
        assert_eq!(get(Arrows, "alt+a"), Some(Action::TableAddRow));
        assert_eq!(get(Vim, "alt+a"), None, "vim adds a row with `a` (plan M2 Deviation 5)");
        assert_eq!(get(Vim, "alt+right"), None, "gt steps the tabs in vim (plan M4a)");
        assert_eq!(get(Vim, "alt+left"), None);
        assert_eq!(get(Arrows, "alt+right"), Some(Action::CycleTabs(1)));
        for combo in [
            "ctrl+c",
            "ctrl+v",
            "ctrl+o",
            "ctrl+p",
            "ctrl+z",
            "alt+t",
            "ctrl+1",
            "alt+p",
            "shift+alt+p",
        ] {
            assert_eq!(
                get(Vim, combo),
                get(Arrows, combo),
                "{combo} is shared until its vim key lands"
            );
        }
        for combo in ["ctrl+s", "alt+w", "shift+alt+w"] {
            assert_eq!(get(Vim, combo), None, "{combo}: an ex verb in vim (plan M4b)");
            assert!(get(Arrows, combo).is_some(), "{combo}");
        }
        assert_eq!(Keymap::default_bindings_for(Arrows), Keymap::default_bindings());
    }

    #[test]
    fn overrides_layer_on_the_profile_they_are_given() {
        let vim = Keymap::try_from_overrides_for(crate::config::KeyProfile::Vim, "save = \"f2\"\n")
            .unwrap();
        assert_eq!(vim.lookup(&KeyCombo::parse("f2").unwrap()), Some(Action::SaveRequest));
        assert_eq!(vim.lookup(&KeyCombo::parse("q").unwrap()), None);
    }

    #[test]
    fn vim_shadow_warnings_name_the_bare_chars_a_vim_row_starts_with() {
        let mut map = Keymap::default_bindings_for(crate::config::KeyProfile::Vim);
        assert!(map.vim_shadow_warnings().is_empty(), "the vim defaults bind no bare char");
        map.apply_overrides("save = [\"ctrl+s\", \"G\"]\nquit = [\"ctrl+c\", \"x\"]\n").unwrap();
        assert_eq!(
            map.vim_shadow_warnings(),
            vec!["keys.toml binds `G`, which shadows vim's `G`".to_string()]
        );
    }

    /// A digit `1`–`9` is a count wherever a `keys.toml` letter can fire in
    /// vim, so a binding on one never runs: the load says so. `0` is a key
    /// while no count is typed, so it is an ordinary shadow.
    #[test]
    fn vim_shadow_warnings_name_a_bound_count_digit() {
        let mut map = Keymap::default_bindings_for(crate::config::KeyProfile::Vim);
        map.apply_overrides("space_3 = [\"ctrl+3\", \"3\"]\nspace_1 = [\"ctrl+1\", \"0\"]\n")
            .unwrap();
        assert_eq!(
            map.vim_shadow_warnings(),
            vec![
                "keys.toml binds `0`, which shadows vim's `0`".to_string(),
                "keys.toml binds `3`, which vim reads as a count, so the binding never runs"
                    .to_string(),
            ]
        );
    }
}
