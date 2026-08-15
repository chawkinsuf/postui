# Stage 1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A running, good-looking TUI shell — workspace skeleton, async event loop, component framework, theme/design-token system, focusable pane layout, keybindings, toasts, modal stack, and command palette — with placeholder pane content.

**Architecture:** Cargo workspace with `postui-core` (headless lib, nearly empty in stage 1) and `postui` (the TUI). The TUI uses the component pattern over an async action loop: crossterm `EventStream` + tick timer + mpsc channel feed a central `Action` enum; `App::update` mutates one state struct; `ui::draw` renders it. All colors come from a `Theme` of named design tokens.

**Tech Stack:** Rust (edition 2024), ratatui 0.30 + crossterm backend, tokio, anyhow, serde/toml, directories.

**Spec:** `docs/superpowers/specs/2026-08-15-postui-design.md` (§2 stack, §5 UI/visual design, §7 architecture, §8 stage 1 row)

## Global Constraints

- Workspace crates: `crates/postui-core` (lib) and `crates/postui` (bin). TUI code never goes in core; core has no terminal/ratatui dependency.
- ratatui `0.30`. Depend on crossterm directly with `features = ["event-stream"]`; the version MUST match ratatui's own crossterm dependency — verify with `cargo tree -i crossterm` (exactly one version in the tree). Import crossterm types via `ratatui::crossterm::...` everywhere.
- No hardcoded `Color::...` values outside `theme.rs`. Every draw call styles via `Theme` tokens.
- No `.unwrap()`/`.expect()` in `postui` app code paths (tests are fine). `main` returns `anyhow::Result<()>`.
- Rounded borders (`BorderType::Rounded`) on all panes; focused pane uses `border_focused` + accent title, unfocused uses `border` + muted title.
- Every task: tests first (TDD), run, implement, run, commit. Commit messages are plain one-liners — no Co-Authored-By, no trailers (user requirement).
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` must pass at every commit.

---

### Task 1: Workspace skeleton

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/postui-core/Cargo.toml`, `crates/postui-core/src/lib.rs`
- Create: `crates/postui/Cargo.toml`, `crates/postui/src/main.rs`
- Create: `.gitignore`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `postui_core::APP_NAME: &str` (used by later stages for config paths); a `postui` bin that compiles and depends on `postui-core`.

- [ ] **Step 1: Write the workspace files**

`Cargo.toml` (root):

```toml
[workspace]
resolver = "2"
members = ["crates/postui-core", "crates/postui"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
```

`crates/postui-core/Cargo.toml`:

```toml
[package]
name = "postui-core"
edition.workspace = true
version.workspace = true

[dependencies]
serde.workspace = true
```

`crates/postui-core/src/lib.rs`:

```rust
/// Working name; final app name TBD (spec header).
pub const APP_NAME: &str = "postui";
```

`crates/postui/Cargo.toml`:

```toml
[package]
name = "postui"
edition.workspace = true
version.workspace = true

[dependencies]
postui-core = { path = "../postui-core" }
anyhow.workspace = true
serde.workspace = true
toml.workspace = true
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
futures = "0.3"
directories = "6"

[dev-dependencies]
```

`crates/postui/src/main.rs`:

```rust
fn main() -> anyhow::Result<()> {
    println!("{}", postui_core::APP_NAME);
    Ok(())
}
```

`.gitignore`:

```
/target
```

- [ ] **Step 2: Verify the crossterm version matches ratatui's**

Run: `cargo tree -i crossterm`
Expected: exactly ONE crossterm version listed, reachable from both `postui` and `ratatui`. If two versions appear, change the direct `crossterm` dependency's version in `crates/postui/Cargo.toml` to the one ratatui uses and re-run until unified.

- [ ] **Step 3: Write a smoke test**

Append to `crates/postui-core/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_nonempty() {
        assert!(!super::APP_NAME.is_empty());
    }
}
```

- [ ] **Step 4: Run checks**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo run -p postui`
Expected: tests pass, clippy clean, binary prints `postui`.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore crates
git commit -m "Scaffold cargo workspace: postui-core lib and postui TUI bin"
```

---

### Task 2: Action enum, App state, terminal lifecycle, minimal run loop

**Files:**
- Create: `crates/postui/src/action.rs`
- Create: `crates/postui/src/app.rs`
- Modify: `crates/postui/src/main.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `enum Action { Quit, Tick, Render }` (extended by later tasks); `struct App { pub should_quit: bool }` with `App::new() -> App` and `App::update(&mut self, action: Action)`; a working `cargo run` that draws a placeholder and quits on `q` or `Ctrl+C`.

- [ ] **Step 1: Write the failing test**

`crates/postui/src/app.rs`:

```rust
use crate::action::Action;

#[derive(Default)]
pub struct App {
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, action: Action) {
        let _ = action;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_action_sets_should_quit() {
        let mut app = App::new();
        assert!(!app.should_quit);
        app.update(Action::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn tick_does_not_quit() {
        let mut app = App::new();
        app.update(Action::Tick);
        assert!(!app.should_quit);
    }
}
```

`crates/postui/src/action.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Tick,
    Render,
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p postui` (add `mod action; mod app;` to `main.rs` first)
Expected: FAIL — `quit_action_sets_should_quit` panics on `todo!()`.

- [ ] **Step 3: Implement update and the run loop**

Replace `App::update`:

```rust
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Tick | Action::Render => {}
        }
    }
```

`crates/postui/src/main.rs`:

```rust
mod action;
mod app;

use action::Action;
use app::App;
use futures::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let terminal = ratatui::init(); // installs a panic hook that restores the terminal
    let result = run(terminal).await;
    ratatui::restore();
    result
}

async fn run(mut terminal: ratatui::DefaultTerminal) -> anyhow::Result<()> {
    let mut app = App::new();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    while !app.should_quit {
        terminal.draw(|frame| {
            frame.render_widget(
                Line::from("postui — press q to quit"),
                frame.area(),
            );
        })?;

        tokio::select! {
            maybe_event = events.next() => {
                if let Some(Ok(event)) = maybe_event {
                    if let Some(action) = map_event(&event) {
                        app.update(action);
                    }
                }
            }
            _ = tick.tick() => app.update(Action::Tick),
        }
    }
    Ok(())
}

fn map_event(event: &Event) -> Option<Action> {
    match event {
        Event::Key(KeyEvent { code: KeyCode::Char('q'), .. }) => Some(Action::Quit),
        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, .. })
            if modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}
```

(`map_event` is temporary; Task 4 replaces it with the keymap.)

- [ ] **Step 4: Run tests and verify manually**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS, clean.
Run: `cargo run -p postui` in a real terminal.
Expected: placeholder text renders; `q` and `Ctrl+C` both exit cleanly with the terminal restored (no raw-mode residue).

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add Action enum, App state, and async event loop with clean terminal lifecycle"
```

---

### Task 3: Theme and design tokens

**Files:**
- Create: `crates/postui/src/theme.rs`
- Modify: `crates/postui/src/main.rs` (add `mod theme;`)

**Interfaces:**
- Consumes: nothing
- Produces:

```rust
pub struct Theme {
    pub surface: Color, pub surface_raised: Color,
    pub text: Color, pub text_muted: Color,
    pub accent: Color,
    pub success: Color, pub error: Color, pub warning: Color,
    pub border: Color, pub border_focused: Color,
}
impl Theme {
    pub fn dark() -> Theme;
    pub fn light() -> Theme;
    pub fn for_terminal() -> Theme;      // dark() today; capability/theme detection later
    pub fn downgrade_to_256(&self) -> Theme; // every token becomes Color::Indexed
}
pub fn rgb_to_indexed(r: u8, g: u8, b: u8) -> u8; // 6x6x6 cube + grayscale ramp
```

- [ ] **Step 1: Write the failing tests**

`crates/postui/src/theme.rs`:

```rust
use ratatui::style::Color;

pub struct Theme {
    pub surface: Color,
    pub surface_raised: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub border: Color,
    pub border_focused: Color,
}

impl Theme {
    pub fn dark() -> Self {
        todo!()
    }

    pub fn light() -> Self {
        todo!()
    }

    pub fn for_terminal() -> Self {
        Self::dark()
    }

    pub fn downgrade_to_256(&self) -> Self {
        todo!()
    }
}

pub fn rgb_to_indexed(_r: u8, _g: u8, _b: u8) -> u8 {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_tokens_are_rgb() {
        let t = Theme::dark();
        for c in [t.surface, t.text, t.accent, t.border, t.border_focused] {
            assert!(matches!(c, Color::Rgb(..)), "token must be truecolor: {c:?}");
        }
    }

    #[test]
    fn focused_border_differs_from_unfocused() {
        let t = Theme::dark();
        assert_ne!(t.border, t.border_focused);
        let t = Theme::light();
        assert_ne!(t.border, t.border_focused);
    }

    #[test]
    fn downgrade_maps_every_token_to_indexed() {
        let t = Theme::dark().downgrade_to_256();
        for c in [
            t.surface, t.surface_raised, t.text, t.text_muted, t.accent,
            t.success, t.error, t.warning, t.border, t.border_focused,
        ] {
            assert!(matches!(c, Color::Indexed(_)), "expected indexed, got {c:?}");
        }
    }

    #[test]
    fn rgb_to_indexed_hits_cube_corners() {
        assert_eq!(rgb_to_indexed(0, 0, 0), 16);      // cube black
        assert_eq!(rgb_to_indexed(255, 255, 255), 231); // cube white
        assert_eq!(rgb_to_indexed(255, 0, 0), 196);   // cube red corner
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui theme` (after adding `mod theme;` to `main.rs`)
Expected: FAIL on `todo!()`.

- [ ] **Step 3: Implement**

Replace the `todo!()` bodies:

```rust
impl Theme {
    /// Starting palette (Tokyo-Night-adjacent); visual direction iterates on
    /// these values during stage-1 polish with the frontend-design skill.
    pub fn dark() -> Self {
        Self {
            surface: Color::Rgb(0x13, 0x17, 0x20),
            surface_raised: Color::Rgb(0x1a, 0x1f, 0x2b),
            text: Color::Rgb(0xd8, 0xde, 0xe9),
            text_muted: Color::Rgb(0x7b, 0x84, 0x96),
            accent: Color::Rgb(0x7a, 0xa2, 0xf7),
            success: Color::Rgb(0x9e, 0xce, 0x6a),
            error: Color::Rgb(0xf7, 0x76, 0x8e),
            warning: Color::Rgb(0xe0, 0xaf, 0x68),
            border: Color::Rgb(0x2a, 0x2f, 0x3a),
            border_focused: Color::Rgb(0x7a, 0xa2, 0xf7),
        }
    }

    pub fn light() -> Self {
        Self {
            surface: Color::Rgb(0xf7, 0xf8, 0xfa),
            surface_raised: Color::Rgb(0xff, 0xff, 0xff),
            text: Color::Rgb(0x24, 0x29, 0x2f),
            text_muted: Color::Rgb(0x6e, 0x77, 0x81),
            accent: Color::Rgb(0x1d, 0x63, 0xed),
            success: Color::Rgb(0x16, 0xa3, 0x4a),
            error: Color::Rgb(0xdc, 0x26, 0x26),
            warning: Color::Rgb(0xd9, 0x77, 0x06),
            border: Color::Rgb(0xd0, 0xd7, 0xde),
            border_focused: Color::Rgb(0x1d, 0x63, 0xed),
        }
    }

    pub fn for_terminal() -> Self {
        Self::dark()
    }

    pub fn downgrade_to_256(&self) -> Self {
        let f = |c: Color| match c {
            Color::Rgb(r, g, b) => Color::Indexed(rgb_to_indexed(r, g, b)),
            other => other,
        };
        Self {
            surface: f(self.surface),
            surface_raised: f(self.surface_raised),
            text: f(self.text),
            text_muted: f(self.text_muted),
            accent: f(self.accent),
            success: f(self.success),
            error: f(self.error),
            warning: f(self.warning),
            border: f(self.border),
            border_focused: f(self.border_focused),
        }
    }
}

/// Nearest xterm-256 color: compares the best 6x6x6 cube match against the
/// best grayscale-ramp match and returns the closer of the two.
pub fn rgb_to_indexed(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_step = |v: u8| -> (u8, u8) {
        let mut best = (0u8, u8::MAX);
        for (i, s) in STEPS.iter().enumerate() {
            let d = v.abs_diff(*s);
            if d < best.1 {
                best = (i as u8, d);
            }
        }
        best
    };
    let (ri, _) = nearest_step(r);
    let (gi, _) = nearest_step(g);
    let (bi, _) = nearest_step(b);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let cube_rgb = (STEPS[ri as usize], STEPS[gi as usize], STEPS[bi as usize]);

    let gray_level = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    let gi2 = (gray_level.saturating_sub(8) / 10).min(23);
    let gray_idx = 232 + gi2;
    let gray_val = 8 + 10 * gi2;

    let dist = |(ar, ag, ab): (u8, u8, u8)| -> u32 {
        let dr = ar.abs_diff(r) as u32;
        let dg = ag.abs_diff(g) as u32;
        let db = ab.abs_diff(b) as u32;
        dr * dr + dg * dg + db * db
    };
    if dist(cube_rgb) <= dist((gray_val, gray_val, gray_val)) {
        cube_idx
    } else {
        gray_idx
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p postui theme && cargo clippy --workspace -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add theme design-token system with dark/light palettes and 256-color fallback"
```

---

### Task 4: Keymap with parseable combos and TOML overrides

**Files:**
- Create: `crates/postui/src/keys.rs`
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Consumes: `Action` from Task 2.
- Produces:

```rust
pub struct KeyCombo { pub code: KeyCode, pub modifiers: KeyModifiers } // PartialEq+Eq+Hash+Clone+Debug
impl KeyCombo {
    pub fn parse(s: &str) -> Option<KeyCombo>;          // "ctrl+p", "shift+tab", "q", "esc"
    pub fn from_event(ev: &KeyEvent) -> KeyCombo;
}
pub struct Keymap { /* HashMap<KeyCombo, Action> */ }
impl Keymap {
    pub fn default_bindings() -> Keymap;
    pub fn lookup(&self, combo: &KeyCombo) -> Option<Action>;
    pub fn apply_overrides(&mut self, toml_str: &str) -> anyhow::Result<()>;
    pub fn load() -> Keymap; // defaults + optional ~/.config/postui/keys.toml
}
```

New `Action` variants: `FocusNext`, `FocusPrev`, `OpenPalette`, `Close`.

- [ ] **Step 1: Extend Action**

In `crates/postui/src/action.rs` add to the enum:

```rust
    FocusNext,
    FocusPrev,
    OpenPalette,
    Close,
```

Add matching no-op arms in `App::update` for now (`Action::FocusNext | Action::FocusPrev | Action::OpenPalette | Action::Close => {}`) — Tasks 5/8/9 implement them.

- [ ] **Step 2: Write the failing tests**

`crates/postui/src/keys.rs`:

```rust
use crate::action::Action;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub fn parse(_s: &str) -> Option<Self> {
        todo!()
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        // SHIFT is implicit in the char itself for printable keys.
        let mods = match ev.code {
            KeyCode::Char(_) => ev.modifiers.difference(KeyModifiers::SHIFT),
            _ => ev.modifiers,
        };
        Self { code: ev.code, modifiers: mods }
    }
}

pub struct Keymap {
    bindings: HashMap<KeyCombo, Action>,
}

impl Keymap {
    pub fn default_bindings() -> Self {
        todo!()
    }

    pub fn lookup(&self, combo: &KeyCombo) -> Option<Action> {
        self.bindings.get(combo).cloned()
    }

    pub fn apply_overrides(&mut self, _toml_str: &str) -> anyhow::Result<()> {
        todo!()
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
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p postui keys` (after `mod keys;` in `main.rs`)
Expected: FAIL on `todo!()`.

- [ ] **Step 4: Implement**

```rust
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
}
```

```rust
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
}
```

Wire into the loop — in `main.rs`, replace `map_event` usage: build `let keymap = Keymap::load();` in `run`, and for `Event::Key(ev)` (only when `ev.kind == KeyEventKind::Press`) do `keymap.lookup(&KeyCombo::from_event(&ev)).map(|a| app.update(a))`. Delete `map_event`.

- [ ] **Step 5: Run tests and verify manually**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS, clean.
Run: `cargo run -p postui` — `q` and `Ctrl+C` still quit.

- [ ] **Step 6: Commit**

```bash
git add crates/postui/src
git commit -m "Add keymap with combo parsing, defaults, and TOML overrides"
```

---

### Task 5: Component trait, pane layout, and focus cycling

**Files:**
- Create: `crates/postui/src/components/mod.rs`
- Create: `crates/postui/src/layout.rs`
- Modify: `crates/postui/src/app.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Consumes: `Action`, `Theme`, `Keymap`.
- Produces:

```rust
// components/mod.rs
pub struct DrawCtx<'a> { pub theme: &'a Theme, pub focused: bool }
pub trait Component {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action>; // default: None
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx);
}
pub fn pane_block<'a>(title: &'a str, ctx: &DrawCtx) -> Block<'a>; // rounded, padded, focus-styled

// layout.rs
pub enum PaneId { Sidebar, Editor, Response } // Copy+Clone+PartialEq+Eq+Debug
impl PaneId { pub fn next(self) -> PaneId; pub fn prev(self) -> PaneId; }
pub struct AppLayout { pub header: Rect, pub sidebar: Rect, pub editor: Rect, pub response: Rect, pub footer: Rect }
pub fn compute_layout(area: Rect) -> AppLayout;

// app.rs additions
pub struct App { pub should_quit: bool, pub focus: PaneId, pub theme: Theme, ... }
```

- [ ] **Step 1: Write the failing tests**

`crates/postui/src/layout.rs`:

```rust
use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Sidebar,
    Editor,
    Response,
}

impl PaneId {
    pub fn next(self) -> Self {
        match self {
            Self::Sidebar => Self::Editor,
            Self::Editor => Self::Response,
            Self::Response => Self::Sidebar,
        }
    }

    pub fn prev(self) -> Self {
        self.next().next() // 3-cycle: two nexts == one prev
    }
}

pub struct AppLayout {
    pub header: Rect,
    pub sidebar: Rect,
    pub editor: Rect,
    pub response: Rect,
    pub footer: Rect,
}

pub fn compute_layout(_area: Rect) -> AppLayout {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycles_through_all_panes_and_back() {
        let start = PaneId::Sidebar;
        let mut p = start;
        let mut seen = vec![p];
        for _ in 0..2 {
            p = p.next();
            seen.push(p);
        }
        assert_eq!(seen, vec![PaneId::Sidebar, PaneId::Editor, PaneId::Response]);
        assert_eq!(p.next(), start);
        assert_eq!(start.prev(), PaneId::Response);
    }

    #[test]
    fn layout_partitions_area() {
        let area = Rect::new(0, 0, 120, 40);
        let l = compute_layout(area);
        assert_eq!(l.header.height, 1);
        assert_eq!(l.footer.height, 1);
        assert_eq!(l.header.y, 0);
        assert_eq!(l.footer.y, 39);
        // sidebar left of editor/response; editor above response
        assert!(l.sidebar.x < l.editor.x);
        assert_eq!(l.editor.x, l.response.x);
        assert!(l.editor.y < l.response.y);
        // body fills between header and footer
        assert_eq!(l.sidebar.y, 1);
        assert_eq!(l.sidebar.height, 38);
        assert_eq!(l.editor.height + l.response.height, 38);
        assert_eq!(l.sidebar.width + l.editor.width, 120);
    }
}
```

`crates/postui/src/components/mod.rs`:

```rust
use crate::action::Action;
use crate::theme::Theme;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Padding};
use ratatui::Frame;

pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx);
}

/// Standard pane chrome: rounded borders, interior padding, focus styling.
pub fn pane_block<'a>(title: &'a str, ctx: &DrawCtx) -> Block<'a> {
    let t = ctx.theme;
    let (border_color, title_color) = if ctx.focused {
        (t.border_focused, t.accent)
    } else {
        (t.border, t.text_muted)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1))
        .title(format!(" {title} "))
        .title_style(Style::default().fg(title_color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn pane_block_renders_rounded_border_and_title() {
        let theme = Theme::dark();
        let ctx = DrawCtx { theme: &theme, focused: true };
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                let b = pane_block("Requests", &ctx);
                f.render_widget(b, f.area());
            })
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains('╭'), "rounded corner expected");
        assert!(content.contains("Requests"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui layout` (add `mod layout; mod components;` to `main.rs`)
Expected: `layout_partitions_area` FAILS on `todo!()`; components test passes already (that is fine — it pins chrome behavior).

- [ ] **Step 3: Implement compute_layout**

```rust
pub fn compute_layout(area: Rect) -> AppLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer hints
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
        .split(rows[1]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[1]);
    AppLayout {
        header: rows[0],
        sidebar: cols[0],
        editor: right[0],
        response: right[1],
        footer: rows[2],
    }
}
```

- [ ] **Step 4: Wire focus into App**

In `app.rs`: add fields `pub focus: PaneId` (init `PaneId::Sidebar`), `pub theme: Theme` (init `Theme::for_terminal()`); in `update` replace the `FocusNext | FocusPrev` no-ops:

```rust
            Action::FocusNext => self.focus = self.focus.next(),
            Action::FocusPrev => self.focus = self.focus.prev(),
```

Add test:

```rust
    #[test]
    fn focus_next_moves_focus() {
        let mut app = App::new();
        let start = app.focus;
        app.update(Action::FocusNext);
        assert_ne!(app.focus, start);
        app.update(Action::FocusPrev);
        assert_eq!(app.focus, start);
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 6: Commit**

```bash
git add crates/postui/src
git commit -m "Add component trait, pane chrome, app layout, and focus cycling"
```

---

### Task 6: Placeholder panes, header bar, footer hints, full-frame draw

**Files:**
- Create: `crates/postui/src/components/sidebar.rs`, `crates/postui/src/components/editor.rs`, `crates/postui/src/components/response.rs`, `crates/postui/src/components/header_bar.rs`, `crates/postui/src/components/footer.rs`
- Create: `crates/postui/src/ui.rs`
- Modify: `crates/postui/src/components/mod.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Consumes: `Component`, `pane_block`, `compute_layout`, `App`.
- Produces: `pub fn draw(frame: &mut Frame, app: &App)` in `ui.rs` — the single full-frame render entry point (later tasks append toast/modal layers to it). `App` gains `pub sidebar: Sidebar, pub editor: Editor, pub response: Response` component fields. Each placeholder component implements `Component` with a helpful empty state.

- [ ] **Step 1: Write the failing test**

`crates/postui/src/ui.rs`:

```rust
use crate::app::App;
use crate::components::{Component, DrawCtx};
use crate::layout::{compute_layout, PaneId};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, app: &App) {
    let _ = (frame, app);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App) -> String {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn full_frame_shows_all_panes_and_chrome() {
        let app = App::new();
        let content = render(&app);
        assert!(content.contains("Requests"));       // sidebar title
        assert!(content.contains("Request"));        // editor title
        assert!(content.contains("Response"));       // response title
        assert!(content.contains("postui"));         // header bar app name
        assert!(content.contains("No environment")); // header env selector placeholder
        assert!(content.contains("quit"));           // footer hint mentions quit key
        assert!(content.contains('╭'));              // rounded chrome
        assert!(content.contains("No project open")); // sidebar empty state
        assert!(content.contains("response will appear here")); // response empty state
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p postui ui` (add `mod ui;` to `main.rs` and `pub mod sidebar; pub mod editor; pub mod response; pub mod header_bar; pub mod footer;` to `components/mod.rs` — create the files as empty-struct stubs first)
Expected: FAIL on `todo!()`.

- [ ] **Step 3: Implement the components**

`crates/postui/src/components/sidebar.rs`:

```rust
use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Sidebar;

impl Component for Sidebar {
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Requests", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("No project open."),
            Line::raw(""),
            Line::raw("Projects and requests"),
            Line::raw("will appear here."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
```

`crates/postui/src/components/editor.rs` (same shape):

```rust
use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Editor;

impl Component for Editor {
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Request", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("Select or create a request to edit it."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
```

`crates/postui/src/components/response.rs`:

```rust
use super::{pane_block, Component, DrawCtx};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

#[derive(Default)]
pub struct Response;

impl Component for Response {
    fn draw(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx) {
        let block = pane_block("Response", ctx);
        let empty = Paragraph::new(vec![
            Line::raw(""),
            Line::raw("Send a request — the response will appear here."),
        ])
        .style(Style::default().fg(ctx.theme.text_muted))
        .centered()
        .block(block);
        frame.render_widget(empty, area);
    }
}
```

`crates/postui/src/components/header_bar.rs` (not a pane — a 1-row bar; not focusable):

```rust
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn draw_header(frame: &mut Frame, area: Rect, theme: &Theme) {
    let line = Line::from(vec![
        Span::styled(" postui ", Style::default().fg(theme.surface).bg(theme.accent).bold()),
        Span::raw("  "),
        Span::styled("env: ", Style::default().fg(theme.text_muted)),
        Span::styled("No environment", Style::default().fg(theme.text_muted).italic()),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}
```

`crates/postui/src/components/footer.rs`:

```rust
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn draw_footer(frame: &mut Frame, area: Rect, theme: &Theme) {
    let hint = |key: &'static str, desc: &'static str| {
        vec![
            Span::styled(format!(" {key} "), Style::default().fg(theme.accent)),
            Span::styled(desc, Style::default().fg(theme.text_muted)),
            Span::raw(" "),
        ]
    };
    let mut spans = Vec::new();
    spans.extend(hint("Tab", "next pane"));
    spans.extend(hint("^P", "palette"));
    spans.extend(hint("q", "quit"));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface_raised)),
        area,
    );
}
```

`ui::draw`:

```rust
pub fn draw(frame: &mut Frame, app: &App) {
    let layout = compute_layout(frame.area());
    crate::components::header_bar::draw_header(frame, layout.header, &app.theme);
    let ctx = |pane: PaneId| DrawCtx { theme: &app.theme, focused: app.focus == pane };
    app.sidebar.draw(frame, layout.sidebar, &ctx(PaneId::Sidebar));
    app.editor.draw(frame, layout.editor, &ctx(PaneId::Editor));
    app.response.draw(frame, layout.response, &ctx(PaneId::Response));
    crate::components::footer::draw_footer(frame, layout.footer, &app.theme);
}
```

In `app.rs` add the fields (`pub sidebar: Sidebar, pub editor: Editor, pub response: Response`, all `Default`). In `main.rs` replace the placeholder `terminal.draw` closure body with `ui::draw(frame, &app)`.

- [ ] **Step 4: Run tests and verify manually**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS.
Run: `cargo run -p postui` — three rounded panes with centered muted empty states, header bar with accent app badge, footer hints; Tab moves the accent border between panes.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add placeholder panes, header bar, footer hints, and full-frame draw"
```

---

### Task 7: Toast notifications

**Files:**
- Create: `crates/postui/src/components/toast.rs`
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/ui.rs`, `crates/postui/src/components/mod.rs`

**Interfaces:**
- Consumes: `Action::Tick` (100 ms cadence from Task 2), `Theme`.
- Produces:

```rust
pub enum ToastKind { Info, Success, Error }
pub struct Toasts { /* private */ }
impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind); // 30-tick (3 s) lifetime
    pub fn on_tick(&mut self);
    pub fn is_empty(&self) -> bool;
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme); // stacked top-right
}
```

New `Action` variant: `ShowToast(String, ToastKind)`. `App` gains `pub toasts: Toasts`; `Action::Tick` now calls `self.toasts.on_tick()`.

- [ ] **Step 1: Write the failing tests**

`crates/postui/src/components/toast.rs`:

```rust
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

const TOAST_LIFETIME_TICKS: u32 = 30; // 3 s at the 100 ms tick

struct Toast {
    message: String,
    kind: ToastKind,
    remaining_ticks: u32,
}

#[derive(Default)]
pub struct Toasts {
    entries: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        let _ = (message.into(), kind);
        todo!()
    }

    pub fn on_tick(&mut self) {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let _ = (frame, screen, theme);
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn toast_expires_after_lifetime_ticks() {
        let mut t = Toasts::default();
        t.push("Saved", ToastKind::Success);
        assert!(!t.is_empty());
        for _ in 0..TOAST_LIFETIME_TICKS - 1 {
            t.on_tick();
        }
        assert!(!t.is_empty(), "alive one tick before expiry");
        t.on_tick();
        assert!(t.is_empty(), "expired at lifetime");
    }

    #[test]
    fn multiple_toasts_expire_independently() {
        let mut t = Toasts::default();
        t.push("first", ToastKind::Info);
        for _ in 0..10 {
            t.on_tick();
        }
        t.push("second", ToastKind::Error);
        for _ in 0..TOAST_LIFETIME_TICKS - 10 {
            t.on_tick();
        }
        assert!(!t.is_empty(), "second toast still alive");
        for _ in 0..10 {
            t.on_tick();
        }
        assert!(t.is_empty());
    }

    #[test]
    fn draw_renders_message_top_right() {
        let mut t = Toasts::default();
        t.push("Copied ✓", ToastKind::Success);
        let theme = Theme::dark();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &theme)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("Copied"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui toast` (add `pub mod toast;` to `components/mod.rs`)
Expected: FAIL on `todo!()`.

- [ ] **Step 3: Implement**

```rust
impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.entries.push(Toast {
            message: message.into(),
            kind,
            remaining_ticks: TOAST_LIFETIME_TICKS,
        });
    }

    pub fn on_tick(&mut self) {
        for t in &mut self.entries {
            t.remaining_ticks = t.remaining_ticks.saturating_sub(1);
        }
        self.entries.retain(|t| t.remaining_ticks > 0);
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let mut y = screen.y + 1;
        for toast in &self.entries {
            let width = (toast.message.chars().count() as u16 + 6).min(screen.width);
            let area = Rect::new(screen.right().saturating_sub(width + 1), y, width, 3);
            if area.bottom() > screen.bottom() {
                break;
            }
            let color = match toast.kind {
                ToastKind::Info => theme.accent,
                ToastKind::Success => theme.success,
                ToastKind::Error => theme.error,
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(color))
                .padding(Padding::horizontal(1))
                .style(Style::default().bg(theme.surface_raised));
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(toast.message.as_str())
                    .style(Style::default().fg(theme.text))
                    .block(block),
                area,
            );
            y += 3;
        }
    }
}
```

Wire up: `Action::ShowToast(String, ToastKind)` variant; in `App::update`:

```rust
            Action::Tick => self.toasts.on_tick(),
            Action::ShowToast(msg, kind) => self.toasts.push(msg, kind),
```

In `ui::draw`, after the footer: `app.toasts.draw(frame, frame.area(), &app.theme);` (toasts layer above panes, below modals).

- [ ] **Step 4: Run tests**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add toast notifications with tick-based expiry"
```

---

### Task 8: Modal stack with dimmed backdrop

**Files:**
- Create: `crates/postui/src/components/modal.rs`
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/ui.rs`, `crates/postui/src/main.rs`, `crates/postui/src/components/mod.rs`

**Interfaces:**
- Consumes: `Action::Close`, `Theme`, `Component` trait.
- Produces:

```rust
pub enum Modal { Message { title: String, body: String } } // Palette variant added in Task 9
pub struct ModalStack { /* private Vec<Modal> */ }
impl ModalStack {
    pub fn push(&mut self, modal: Modal);
    pub fn pop(&mut self) -> Option<Modal>;
    pub fn is_empty(&self) -> bool;
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action>; // routes to TOP modal only
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme);
}
pub fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect;
pub fn dim_backdrop(frame: &mut Frame, screen: Rect);
```

Key routing contract (implemented in `main.rs` this task): if `!app.modals.is_empty()`, key events go to `app.modals.handle_key` INSTEAD of the global keymap; `Action::Close` pops the top modal when the stack is non-empty, else does nothing.

- [ ] **Step 1: Write the failing tests**

`crates/postui/src/components/modal.rs`:

```rust
use crate::action::Action;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

pub enum Modal {
    Message { title: String, body: String },
}

#[derive(Default)]
pub struct ModalStack {
    stack: Vec<Modal>,
}

impl ModalStack {
    pub fn push(&mut self, modal: Modal) {
        self.stack.push(modal);
    }

    pub fn pop(&mut self) -> Option<Modal> {
        self.stack.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        todo!()
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let _ = (frame, screen, theme);
        todo!()
    }
}

pub fn centered_rect(_screen: Rect, _width: u16, _height: u16) -> Rect {
    todo!()
}

pub fn dim_backdrop(frame: &mut Frame, screen: Rect) {
    frame
        .buffer_mut()
        .set_style(screen, Style::default().add_modifier(Modifier::DIM));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn centered_rect_is_centered_and_clamped() {
        let screen = Rect::new(0, 0, 100, 40);
        let r = centered_rect(screen, 60, 10);
        assert_eq!(r, Rect::new(20, 15, 60, 10));
        let clamped = centered_rect(screen, 200, 90);
        assert_eq!(clamped.width, 100);
        assert_eq!(clamped.height, 40);
    }

    #[test]
    fn esc_closes_top_modal_only() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "A".into(), body: "a".into() });
        m.push(Modal::Message { title: "B".into(), body: "b".into() });
        let action = m.handle_key(key(KeyCode::Esc));
        assert_eq!(action, Some(Action::Close));
    }

    #[test]
    fn other_keys_are_swallowed_by_message_modal() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "A".into(), body: "a".into() });
        assert_eq!(m.handle_key(key(KeyCode::Char('q'))), None,
            "keys must not leak through a modal to global bindings");
    }

    #[test]
    fn draw_renders_title_and_body() {
        let mut m = ModalStack::default();
        m.push(Modal::Message { title: "About".into(), body: "hello world".into() });
        let theme = Theme::dark();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| m.draw(f, f.area(), &theme)).unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("About"));
        assert!(content.contains("hello world"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui modal` (add `pub mod modal;` to `components/mod.rs`; derive `PartialEq` is already on `Action`)
Expected: FAIL on `todo!()`.

- [ ] **Step 3: Implement**

```rust
impl ModalStack {
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let top = self.stack.last_mut()?;
        match top {
            Modal::Message { .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter => Some(Action::Close),
                _ => None, // swallowed: modals capture all input
            },
        }
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let Some(top) = self.stack.last() else { return };
        dim_backdrop(frame, screen);
        match top {
            Modal::Message { title, body } => {
                let area = centered_rect(screen, 60.min(screen.width), 9);
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(theme.border_focused))
                    .padding(Padding::uniform(1))
                    .style(Style::default().bg(theme.surface_raised))
                    .title(format!(" {title} "))
                    .title_style(Style::default().fg(theme.accent));
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(body.as_str())
                        .style(Style::default().fg(theme.text))
                        .wrap(Wrap { trim: false })
                        .block(block),
                    area,
                );
            }
        }
    }
}

pub fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(screen.width);
    let h = height.min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}
```

Wire up. `App` gains `pub modals: ModalStack`. In `App::update`:

```rust
            Action::Close => {
                let _ = self.modals.pop(); // no-op when empty
            }
```

In `main.rs` key handling, route through modals first:

```rust
                if let Event::Key(ev) = event {
                    if ev.kind == KeyEventKind::Press {
                        let action = if !app.modals.is_empty() {
                            app.modals.handle_key(ev)
                        } else {
                            keymap.lookup(&KeyCombo::from_event(&ev))
                        };
                        if let Some(action) = action {
                            app.update(action);
                        }
                    }
                }
```

In `ui::draw`, render LAST (above toasts): `app.modals.draw(frame, frame.area(), &app.theme);`

Add an app-level routing test in `app.rs`:

```rust
    #[test]
    fn close_pops_modal_instead_of_quitting() {
        use crate::components::modal::Modal;
        let mut app = App::new();
        app.modals.push(Modal::Message { title: "t".into(), body: "b".into() });
        app.update(Action::Close);
        assert!(app.modals.is_empty());
        assert!(!app.should_quit);
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add modal stack with dimmed backdrop and input capture"
```

---

### Task 9: Command palette

**Files:**
- Create: `crates/postui/src/components/palette.rs`
- Modify: `crates/postui/src/components/modal.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- Consumes: `Modal`/`ModalStack` (Task 8), `Action`.
- Produces:

```rust
pub struct Command { pub name: &'static str, pub action: Action }
pub fn all_commands() -> Vec<Command>; // stage-1 set: quit, focus panes, show about
pub struct PaletteState { /* input, selected, filtered */ }
impl PaletteState {
    pub fn new() -> PaletteState;
    pub fn input(&self) -> &str;
    pub fn filtered(&self) -> &[Command];
    pub fn selected(&self) -> usize;
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action>; // typing filters; Enter dispatches; Esc closes
    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme);
}
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool; // case-insensitive subsequence
```

`Modal` gains a `Palette(PaletteState)` variant; `Action::OpenPalette` pushes it. New `Action` variants: `FocusPane(PaneId)`, `ShowAbout`.

- [ ] **Step 1: Extend Action and Modal**

`action.rs`: add `FocusPane(crate::layout::PaneId)` and `ShowAbout`. In `App::update`:

```rust
            Action::FocusPane(pane) => self.focus = pane,
            Action::OpenPalette => self.modals.push(Modal::Palette(PaletteState::new())),
            Action::ShowAbout => self.modals.push(Modal::Message {
                title: "postui".into(),
                body: "A fast, local-first terminal HTTP client.".into(),
            }),
```

(Palette-dispatched actions arrive via `update`, so dispatching `FocusPane` must ALSO close the palette — see Step 3 wiring.)

- [ ] **Step 2: Write the failing tests**

`crates/postui/src/components/palette.rs`:

```rust
use crate::action::Action;
use crate::layout::PaneId;
use crate::theme::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph};
use ratatui::Frame;

#[derive(Clone)]
pub struct Command {
    pub name: &'static str,
    pub action: Action,
}

pub fn all_commands() -> Vec<Command> {
    vec![
        Command { name: "Focus: request tree", action: Action::FocusPane(PaneId::Sidebar) },
        Command { name: "Focus: request editor", action: Action::FocusPane(PaneId::Editor) },
        Command { name: "Focus: response", action: Action::FocusPane(PaneId::Response) },
        Command { name: "Help: about postui", action: Action::ShowAbout },
        Command { name: "Quit", action: Action::Quit },
    ]
}

pub fn fuzzy_match(_needle: &str, _haystack: &str) -> bool {
    todo!()
}

pub struct PaletteState {
    input: String,
    selected: usize,
    filtered: Vec<Command>,
}

impl PaletteState {
    pub fn new() -> Self {
        Self { input: String::new(), selected: 0, filtered: all_commands() }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn filtered(&self) -> &[Command] {
        &self.filtered
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn refilter(&mut self) {
        todo!()
    }

    pub fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        todo!()
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let _ = (frame, screen, theme);
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn fuzzy_match_is_case_insensitive_subsequence() {
        assert!(fuzzy_match("fre", "Focus: request editor"));
        assert!(fuzzy_match("QUIT", "Quit"));
        assert!(fuzzy_match("", "anything"));
        assert!(!fuzzy_match("xyz", "Quit"));
    }

    #[test]
    fn typing_filters_and_backspace_restores() {
        let mut p = PaletteState::new();
        let total = p.filtered().len();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.filtered().len(), 1);
        assert_eq!(p.filtered()[0].name, "Quit");
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.filtered().len(), total);
    }

    #[test]
    fn arrows_move_selection_within_bounds() {
        let mut p = PaletteState::new();
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Up)); // clamped at top
        assert_eq!(p.selected(), 0);
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.selected(), 1);
    }

    #[test]
    fn enter_returns_selected_action() {
        let mut p = PaletteState::new();
        for c in "quit".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.handle_key(key(KeyCode::Enter)), Some(Action::Quit));
    }

    #[test]
    fn enter_on_empty_results_does_nothing() {
        let mut p = PaletteState::new();
        for c in "zzzz".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert!(p.filtered().is_empty());
        assert_eq!(p.handle_key(key(KeyCode::Enter)), None);
    }

    #[test]
    fn selection_resets_when_filter_changes() {
        let mut p = PaletteState::new();
        p.handle_key(key(KeyCode::Down));
        p.handle_key(key(KeyCode::Char('q')));
        assert_eq!(p.selected(), 0);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail, then implement**

Run: `cargo test -p postui palette` (add `pub mod palette;` to `components/mod.rs`)
Expected: FAIL on `todo!()`. Then implement:

```rust
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let needle = needle.to_lowercase();
    let haystack = haystack.to_lowercase();
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

impl PaletteState {
    fn refilter(&mut self) {
        self.filtered = all_commands()
            .into_iter()
            .filter(|c| fuzzy_match(&self.input, c.name))
            .collect();
        self.selected = 0;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => return Some(Action::Close),
            KeyCode::Enter => {
                return self.filtered.get(self.selected).map(|c| c.action.clone());
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refilter();
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                self.refilter();
            }
            _ => {}
        }
        None
    }

    pub fn draw(&self, frame: &mut Frame, screen: Rect, theme: &Theme) {
        let width = 50.min(screen.width);
        let height = (self.filtered.len() as u16 + 4).clamp(5, 16).min(screen.height);
        let area = super::modal::centered_rect(screen, width, height);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focused))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(theme.surface_raised))
            .title(" Commands ")
            .title_style(Style::default().fg(theme.accent));
        frame.render_widget(Clear, area);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let prompt = Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(self.input.as_str(), Style::default().fg(theme.text)),
            Span::styled("▏", Style::default().fg(theme.accent)),
        ]);
        let prompt_area = Rect { height: 1, ..inner };
        frame.render_widget(Paragraph::new(prompt), prompt_area);

        let list_area = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let style = if i == self.selected {
                    Style::default().fg(theme.accent).bold()
                } else {
                    Style::default().fg(theme.text)
                };
                let marker = if i == self.selected { "› " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, style),
                    Span::styled(c.name, style),
                ]))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
    }
}
```

Wire into `Modal` (in `modal.rs`):

```rust
pub enum Modal {
    Message { title: String, body: String },
    Palette(crate::components::palette::PaletteState),
}
```

In `ModalStack::handle_key`, add the arm (palette actions close the palette before dispatching, EXCEPT `Close` which just pops):

```rust
            Modal::Palette(state) => state.handle_key(key),
```

and in `ModalStack::draw`:

```rust
            Modal::Palette(state) => state.draw(frame, screen, theme),
```

In `App::update`, palette-dispatched actions must dismiss the palette: change the `main.rs` modal routing so that when `app.modals.handle_key(ev)` returns `Some(action)` and the action is NOT `Action::Close`, pop the modal first, then `app.update(action)`; for `Action::Close` just `app.update(action)`. Concretely in `main.rs`:

```rust
                        if let Some(action) = action {
                            if !app.modals.is_empty() && action != Action::Close {
                                let _ = app.modals.pop();
                            }
                            app.update(action);
                        }
```

Add regression test in `app.rs`:

```rust
    #[test]
    fn open_palette_pushes_modal() {
        let mut app = App::new();
        app.update(Action::OpenPalette);
        assert!(!app.modals.is_empty());
    }
```

- [ ] **Step 4: Run tests and verify manually**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS.
Run: `cargo run -p postui` — Ctrl+P opens a centered palette over a dimmed backdrop; typing filters; Enter on "Focus: response" moves focus and closes; Esc closes; "Help: about postui" opens the message modal.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add command palette with fuzzy filtering"
```

---

### Task 10: Mouse support — click to focus, hover scroll routing stub

**Files:**
- Modify: `crates/postui/src/layout.rs`, `crates/postui/src/main.rs`

**Interfaces:**
- Consumes: `compute_layout`, `AppLayout`, `PaneId`, `Action::FocusPane`.
- Produces: `pub fn hit_test(layout: &AppLayout, x: u16, y: u16) -> Option<PaneId>`; mouse capture enabled in the terminal lifecycle; clicking a pane focuses it. (Scroll events are hit-tested and dropped for now — placeholder panes have nothing to scroll; the contract that scroll routes to the HOVERED pane is established here for stage 2 to implement.)

- [ ] **Step 1: Write the failing test**

Append to `crates/postui/src/layout.rs`:

```rust
pub fn hit_test(layout: &AppLayout, x: u16, y: u16) -> Option<PaneId> {
    let _ = (layout, x, y);
    todo!()
}
```

and to its tests:

```rust
    #[test]
    fn hit_test_maps_coordinates_to_panes() {
        let layout = compute_layout(Rect::new(0, 0, 120, 40));
        let center = |r: Rect| (r.x + r.width / 2, r.y + r.height / 2);
        let (x, y) = center(layout.sidebar);
        assert_eq!(hit_test(&layout, x, y), Some(PaneId::Sidebar));
        let (x, y) = center(layout.editor);
        assert_eq!(hit_test(&layout, x, y), Some(PaneId::Editor));
        let (x, y) = center(layout.response);
        assert_eq!(hit_test(&layout, x, y), Some(PaneId::Response));
        // header row is not a pane
        assert_eq!(hit_test(&layout, 5, 0), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p postui hit_test`
Expected: FAIL on `todo!()`.

- [ ] **Step 3: Implement**

```rust
pub fn hit_test(layout: &AppLayout, x: u16, y: u16) -> Option<PaneId> {
    let pos = ratatui::layout::Position { x, y };
    if layout.sidebar.contains(pos) {
        Some(PaneId::Sidebar)
    } else if layout.editor.contains(pos) {
        Some(PaneId::Editor)
    } else if layout.response.contains(pos) {
        Some(PaneId::Response)
    } else {
        None
    }
}
```

In `main.rs`:
- After `ratatui::init()`, enable mouse capture:

```rust
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEvent, MouseEventKind};
use ratatui::crossterm::execute;

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = run(&mut terminal).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
```

- In the event arm, handle mouse events (modals swallow clicks for now):

```rust
                Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column, row, .. }) => {
                    if app.modals.is_empty() {
                        let layout = compute_layout(terminal.get_frame().area());
                        if let Some(pane) = hit_test(&layout, column, row) {
                            app.update(Action::FocusPane(pane));
                        }
                    }
                }
```

(If `terminal.get_frame()` borrows conflict, compute from `terminal.size()?` via `Rect::new(0, 0, size.width, size.height)` instead.)

- [ ] **Step 4: Run tests and verify manually**

Run: `cargo test -p postui && cargo clippy --workspace -- -D warnings`
Expected: PASS.
Run: `cargo run -p postui` — clicking each pane moves the accent border to it; clicks while the palette is open do nothing.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src
git commit -m "Add mouse capture with click-to-focus pane hit testing"
```

---

### Task 11: Stage-1 acceptance — full-frame integration test and startup toast

**Files:**
- Create: `crates/postui/tests/stage1_acceptance.rs` — requires making the bin also a lib: add `crates/postui/src/lib.rs` re-exporting the modules, and slim `main.rs` to call into it
- Modify: `crates/postui/src/main.rs`, `crates/postui/src/lib.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `postui` as bin+lib (`lib.rs` declares `pub mod action; pub mod app; pub mod components; pub mod keys; pub mod layout; pub mod theme; pub mod ui;`); an integration test that drives the app through actions and asserts rendered frames; a one-time welcome toast on startup proving the toast layer end-to-end.

- [ ] **Step 1: Restructure bin → bin+lib**

Create `crates/postui/src/lib.rs`:

```rust
pub mod action;
pub mod app;
pub mod components;
pub mod keys;
pub mod layout;
pub mod theme;
pub mod ui;
```

`main.rs` keeps ONLY `main`, `run`, and event mapping; its `mod x;` declarations become `use postui::{action::Action, app::App, ...};`. Everything else moves unchanged.

- [ ] **Step 2: Write the failing integration test**

`crates/postui/tests/stage1_acceptance.rs`:

```rust
use postui::action::Action;
use postui::app::App;
use postui::components::toast::ToastKind;
use postui::layout::PaneId;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| postui::ui::draw(f, app)).unwrap();
    format!("{:?}", terminal.backend().buffer())
}

#[test]
fn stage1_acceptance_flow() {
    let mut app = App::new();

    // 1. Initial frame: all chrome present, sidebar focused.
    let frame = render(&app);
    assert!(frame.contains("Requests") && frame.contains("Response"));

    // 2. Focus cycling reaches every pane.
    app.update(Action::FocusNext);
    assert_eq!(app.focus, PaneId::Editor);
    app.update(Action::FocusNext);
    assert_eq!(app.focus, PaneId::Response);

    // 3. Toast renders and expires.
    app.update(Action::ShowToast("Welcome to postui".into(), ToastKind::Info));
    assert!(render(&app).contains("Welcome to postui"));
    for _ in 0..40 {
        app.update(Action::Tick);
    }
    assert!(!render(&app).contains("Welcome to postui"));

    // 4. Palette opens as a modal and renders.
    app.update(Action::OpenPalette);
    assert!(!app.modals.is_empty());
    assert!(render(&app).contains("Commands"));
    app.update(Action::Close);
    assert!(app.modals.is_empty());

    // 5. About modal via its action.
    app.update(Action::ShowAbout);
    assert!(render(&app).contains("local-first"));
    app.update(Action::Close);

    // 6. Quit.
    app.update(Action::Quit);
    assert!(app.should_quit);
}
```

- [ ] **Step 3: Run test to verify it fails, then fix compilation**

Run: `cargo test -p postui --test stage1_acceptance`
Expected: compile errors until the lib restructure is complete and all listed modules are `pub`; then PASS. Fix visibility (`pub`) where the test needs access — public fields on `App` used above must be `pub`.

- [ ] **Step 4: Add the startup toast**

In `run` in `main.rs`, before the loop:

```rust
    app.update(Action::ShowToast(
        "Welcome to postui".into(),
        postui::components::toast::ToastKind::Info,
    ));
```

- [ ] **Step 5: Run everything**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all tests pass, clippy clean.
Run: `cargo run -p postui` — full manual sweep: welcome toast appears top-right and fades ~3 s; Tab/Shift+Tab and mouse clicks move focus; Ctrl+P palette works end-to-end; Esc layering correct; `q` quits clean on Linux. If a macOS machine is available, repeat there (`cargo build` cross-check at minimum is deferred to CI in a later stage).

- [ ] **Step 6: Commit**

```bash
git add crates/postui
git commit -m "Restructure postui as bin+lib and add stage-1 acceptance test with startup toast"
```

---

## Deviations & API drift

ratatui 0.30 is newer than some of this plan's API assumptions (e.g. `ratatui::init`, `Padding`, `Position::contains`, `TestBackend` buffer Debug format, crossterm 0.29 version pin). If a call in this plan doesn't exist under the installed version: check the ratatui 0.30 docs (docs.rs/ratatui/0.30) for the renamed equivalent and use it — that is API drift, not a design change, and needs no user approval. Anything that would change an interface another task consumes (names/types in an **Interfaces** block) DOES require stopping and surfacing per the user's global conflict rule.
