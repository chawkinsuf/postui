# Stage 5: Painted UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reskin postui from line-border text idioms to painted GUI-style controls (filled surfaces, bevels, padded pills, focus rings) driven by a seed→generator theme engine that can adopt the terminal's own color scheme.

**Architecture:** A new pure paint layer (`crates/postui/src/paint/`) renders every control kind onto the ratatui `Buffer` from `(area, state, tokens)`; existing components keep their logic and hit-testing and delegate all drawing to it. `theme.rs` becomes a seeds→generator→tokens engine with optional OSC terminal-color seeding behind a testable trait. Components are reskinned one at a time; legacy idioms (`pane_block`, `hit::button` brackets) are deleted at the end.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29 (via `ratatui::crossterm`), no new runtime deps (OSC queries are hand-rolled over crossterm).

**Spec:** `docs/superpowers/specs/2026-08-16-stage5-painted-ui-design.md`

## Global Constraints

- `export PATH="$HOME/.cargo/bin:$PATH"` before every cargo command (sandbox PATH lacks rustup).
- Import crossterm types via `ratatui::crossterm::...`; never add a second crossterm version (`cargo tree -i crossterm` must show exactly one).
- Every stage 1–4 acceptance test keeps passing at every commit; tests asserting old visual idioms are UPDATED to assert the new idiom, not deleted.
- All colors come from `Theme` tokens — no literal `Color::Rgb` outside `theme/`.
- Glyph vocabulary: bevels `▔`/`▁`, pill pads `▄`/`▀`, accent bars `█`/`▌`-free (full-block `█` on text rows, `▄`/`▀` caps), column divider `▏`, active-cell bar `▎`, chevrons `⌄`/`›`, dropdown `▾`.
- Run the visual check after each reskin task: tmux + capture recipe below ("Visual verification").

## Visual verification (used by several tasks)

Hold a tmux server in a background Bash call, then drive and capture:

```bash
export TMUX_TMPDIR=/tmp/claude-1000/tmux
tmux kill-server 2>/dev/null
tmux new-session -d -s postui -x 200 -y 50 "$PWD/target/debug/postui" && sleep 3600   # run_in_background: true
# later calls:
export TMUX_TMPDIR=/tmp/claude-1000/tmux
tmux capture-pane -t postui:0 -e -p > /tmp/claude-1000/cap.ansi
```

Render `cap.ansi` to PNG with the session ansi2png script (scratchpad `ansi2png.py`; recreate from the tmux-tui-driving memory if missing) and READ the PNG. Compare against the approved mocks (`mock-main.png`, `mock-modal.png`, `mock-palette.png` in the session scratchpad; regenerate via `mock_stage5.py` if gone).

---

### Task 1: Theme engine — seeds, generator, tokens

**Files:**
- Create: `crates/postui/src/theme/mod.rs` (moves current `theme.rs` content, then rebuilds)
- Delete: `crates/postui/src/theme.rs` (becomes the module dir)
- Test: unit tests inside `crates/postui/src/theme/mod.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces (later tasks rely on these exact names):

```rust
pub struct Seeds { pub bg: (u8,u8,u8), pub fg: (u8,u8,u8), pub accent: (u8,u8,u8),
                   pub success: (u8,u8,u8), pub warning: (u8,u8,u8), pub error: (u8,u8,u8) }
impl Seeds { pub fn dark() -> Self; pub fn light() -> Self; }

pub struct Theme {
    dark: bool,
    // surface ladder
    pub page: Color, pub panel: Color, pub control: Color,
    pub control_hover: Color, pub control_pressed: Color,
    // bevel pair (relative to `control`; paint layer derives per-surface variants)
    pub edge_light: Color, pub edge_dark: Color,
    // accent family
    pub accent: Color, pub accent_edge_light: Color, pub accent_edge_dark: Color,
    pub on_accent: Color, pub focus_ring: Color,
    // text
    pub text: Color, pub text_muted: Color, pub text_disabled: Color,
    // semantics (kept from stage 4)
    pub success: Color, pub warning: Color, pub error: Color,
    // legacy aliases kept until Task 12 removes them:
    pub surface: Color, pub surface_raised: Color, pub border: Color, pub border_focused: Color,
}
impl Theme {
    pub fn generate(seeds: &Seeds) -> Self;
    pub fn dark() -> Self;              // Self::generate(&Seeds::dark())
    pub fn light() -> Self;             // Self::generate(&Seeds::light())
    pub fn for_terminal() -> Self;      // still Self::dark(); Task 2 rewires
    pub fn is_dark(&self) -> bool;
    pub fn method_color(&self, m: postui_core::model::Method) -> Color;   // unchanged mapping
    pub fn status_color(&self, status: u16) -> Color;                     // 2xx success, 3xx accent, else error
    pub fn tint(&self, c: Color, surface: Color) -> Color;                // 22% blend for chip fills
    pub fn downgrade_to_256(&self) -> Self;                               // maps every token
}
pub fn rgb_to_indexed(r: u8, g: u8, b: u8) -> u8;   // unchanged
```

Generator rules (lightness math on linear-ish RGB is sufficient; implement `fn lift(rgb, delta_l)` that converts to Oklab L, adds `delta_l`, converts back — a compact self-contained Oklab conversion, ~30 lines, is written in this task):
- dark seeds (bg L < 0.5): page = bg; panel = lift(bg, +0.03); control = lift(bg, +0.06); hover = lift(bg, +0.10); pressed = lift(bg, −0.02); edge_light = lift(control, +0.08); edge_dark = lift(control, −0.08).
- light seeds: same magnitudes, negated (ladder descends).
- accent_edge_light = lift(accent, +0.12); accent_edge_dark = lift(accent, −0.12); on_accent = white if accent L < 0.6 else near-black; focus_ring = accent.
- text = fg; text_muted = blend(fg, bg, 0.55); text_disabled = blend(fg, bg, 0.35).
- Contrast clamp: if Oklab ΔL(text, page) < 0.4, push text away from bg until ≥ 0.4.
- Legacy aliases: surface = page, surface_raised = panel, border = edge_light, border_focused = accent.

- [ ] **Step 1: Write the failing tests**

Move `theme.rs` → `theme/mod.rs` unchanged first (`git mv`), confirm `cargo test -p postui` still passes, commit "refactor: theme.rs to module dir". Then add tests:

```rust
#[test]
fn generator_ladder_is_monotonic_dark() {
    let t = Theme::dark();
    let l = |c: Color| oklab_l(rgb_of(c)); // test helpers over the same conversion fns
    assert!(l(t.page) < l(t.panel));
    assert!(l(t.panel) < l(t.control));
    assert!(l(t.control) < l(t.control_hover));
    assert!(l(t.control_pressed) < l(t.control));
}

#[test]
fn generator_ladder_inverts_for_light_seeds() {
    let t = Theme::light();
    let l = |c: Color| oklab_l(rgb_of(c));
    assert!(l(t.page) > l(t.panel));
    assert!(l(t.panel) > l(t.control));
}

#[test]
fn text_contrast_is_clamped() {
    // pathological seeds: fg nearly equal to bg
    let s = Seeds { fg: (30, 30, 34), ..Seeds::dark() };
    let t = Theme::generate(&s);
    assert!((oklab_l(rgb_of(t.text)) - oklab_l(rgb_of(t.page))).abs() >= 0.4);
}

#[test]
fn status_color_classes() {
    let t = Theme::dark();
    assert_eq!(t.status_color(200), t.success);
    assert_eq!(t.status_color(301), t.accent);
    assert_eq!(t.status_color(404), t.error);
    assert_eq!(t.status_color(500), t.error);
}

#[test]
fn downgrade_maps_every_new_token_to_indexed() {
    let t = Theme::dark().downgrade_to_256();
    for c in [t.page, t.panel, t.control, t.control_hover, t.control_pressed,
              t.edge_light, t.edge_dark, t.accent_edge_light, t.accent_edge_dark,
              t.on_accent, t.focus_ring, t.text_disabled] {
        assert!(matches!(c, Color::Indexed(_)));
    }
}
```

Keep the existing tests (`method_color…`, `rgb_to_indexed…`, `variant_is_reported…`, `focused_border_differs…`) — they must still pass against the generated tokens.

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p postui theme`
Expected: FAIL — `generate`, `page`, `status_color` etc. not defined.

- [ ] **Step 3: Implement Seeds, Oklab helpers, generator**

Write `fn oklab_l((u8,u8,u8)) -> f32`, `fn lift((u8,u8,u8), f32) -> (u8,u8,u8)`, `fn blend(a, b, t)` privately in `theme/mod.rs` (standard Oklab: srgb→linear→LMS→cube-root→L; invert for the way back), then `Theme::generate` per the generator rules above, and rewrite `dark()`/`light()` as seed calls. Keep `for_terminal()` returning `dark()` for now. Add `status_color`, `tint`. Extend `downgrade_to_256` over every field.

- [ ] **Step 4: Run all postui tests**

Run: `cargo test -p postui`
Expected: PASS (legacy alias fields keep every consumer compiling; e.g. `surface` = `page`).

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src/theme
git commit -m "feat: seed->generator->tokens theme engine with surface ladder"
```

---

### Task 2: Terminal color query (OSC) + theme selection config

**Files:**
- Create: `crates/postui/src/theme/osc.rs`
- Modify: `crates/postui/src/theme/mod.rs` (add `Theme::from_environment`)
- Modify: `crates/postui/src/config.rs` (add `theme` key)
- Modify: `crates/postui/src/main.rs` + `crates/postui/src/app.rs:313` (use `from_environment`)

**Interfaces:**
- Consumes: `Seeds`, `Theme::generate` from Task 1.
- Produces:

```rust
// theme/osc.rs
pub struct QueriedColors { pub bg: Option<(u8,u8,u8)>, pub fg: Option<(u8,u8,u8)>,
                           pub ansi: [Option<(u8,u8,u8)>; 16] }
pub trait TerminalPalette { fn query(&mut self) -> QueriedColors; }
pub struct OscQuery;              // real impl over stdout/stdin
impl TerminalPalette for OscQuery { /* … */ }

// theme/mod.rs
pub enum ThemeChoice { Terminal, Dark, Light }   // parsed from config, default Terminal
impl Theme {
    pub fn from_environment(choice: ThemeChoice, term: &mut dyn TerminalPalette) -> Theme;
}
```

`from_environment` logic: `Dark`/`Light` → built-ins. `Terminal` → `term.query()`; if `bg` present, build `Seeds { bg, fg: fg.or(derived-from-bg-luminance), accent: ansi[4].or(ansi[12]).unwrap_or(builtin), success: ansi[2]…, warning: ansi[3]…, error: ansi[1]… }` and `generate`; if `bg` absent → `Seeds::dark()`. Config: `theme = "terminal" | "dark" | "light"` (unknown value → Terminal + startup toast handled by existing config-warning path).

`OscQuery::query` implementation sketch (this is the whole trick — raw mode is already active at call time in `main.rs`; do NOT enable/disable it here):

```rust
use ratatui::crossterm::event::{poll, read, Event, KeyCode};   // NOT used; we read raw bytes
use std::io::{Read, Write};
// Write queries: OSC 10 (fg), OSC 11 (bg), OSC 4;n for n in [1,2,3,4,9,10,11,12]:
//   "\x1b]10;?\x07\x1b]11;?\x07\x1b]4;1;?\x07…"  then a DA1 fence "\x1b[c".
// Read from stdin with a 150ms deadline until the DA1 reply ("\x1b[?…c") arrives;
// parse any "\x1b]{code};rgb:RRRR/GGGG/BBBB" responses seen on the way (16-bit per
// channel; take the high byte). Everything else is discarded. tmux passes these
// through. A terminal that answers nothing hits the deadline and returns all-None.
```

- [ ] **Step 1: Write the failing tests**

In `theme/osc.rs` tests: a `FakePalette` struct implementing `TerminalPalette` returning canned values; plus a pure parser function so the byte-level protocol is testable without a TTY:

```rust
pub fn parse_osc_response(buf: &[u8]) -> QueriedColors;   // exposed for tests

#[test]
fn parses_osc11_bg_reply() {
    let q = parse_osc_response(b"\x1b]11;rgb:1e1e/2a2a/3939\x07\x1b[?6c");
    assert_eq!(q.bg, Some((0x1e, 0x2a, 0x39)));
}
#[test]
fn parses_osc4_slot() {
    let q = parse_osc_response(b"\x1b]4;4;rgb:0101/7878/d4d4\x07");
    assert_eq!(q.ansi[4], Some((0x01, 0x78, 0xd4)));
}
#[test]
fn empty_input_yields_all_none() {
    let q = parse_osc_response(b"");
    assert!(q.bg.is_none() && q.fg.is_none() && q.ansi.iter().all(Option::is_none));
}

// theme/mod.rs tests
#[test]
fn from_environment_seeds_from_terminal_answer() {
    struct Fake(QueriedColors);
    impl TerminalPalette for Fake { fn query(&mut self) -> QueriedColors { /* clone self.0 */ } }
    let mut f = Fake(QueriedColors { bg: Some((16,16,20)), fg: Some((226,226,230)),
                                     ansi: { let mut a=[None;16]; a[4]=Some((1,120,212)); a } });
    let t = Theme::from_environment(ThemeChoice::Terminal, &mut f);
    assert_eq!(t.page, Color::Rgb(16,16,20));
    assert_eq!(t.accent, Color::Rgb(1,120,212));
}
#[test]
fn from_environment_falls_back_to_dark_when_silent() {
    struct Silent;
    impl TerminalPalette for Silent { fn query(&mut self) -> QueriedColors { QueriedColors { bg: None, fg: None, ansi: [None;16] } } }
    let t = Theme::from_environment(ThemeChoice::Terminal, &mut Silent);
    assert_eq!(t.page, Theme::dark().page);
}
```

Config test (in `config.rs` alongside existing ones): `theme = "light"` round-trips to `ThemeChoice::Light`; missing key defaults to `ThemeChoice::Terminal`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui theme:: config::`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement parser, OscQuery, from_environment, config key**

Implement per the sketches. In `main.rs`, after raw mode is enabled and before the first draw, replace the `Theme::for_terminal()` call site (`app.rs:313` receives the theme via the existing constructor path) with `Theme::from_environment(config.theme_choice(), &mut OscQuery)`. Keep `Theme::for_terminal()` as `dark()` for tests that construct `App` directly.

- [ ] **Step 4: Run all tests, then a live sanity run**

Run: `cargo test -p postui` → PASS.
Then `cargo build` and launch in the held tmux session (Visual verification recipe) — the app must start under tmux (query answered or 150ms fallback) with no visible startup delay or leaked escape bytes on screen.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src/theme crates/postui/src/config.rs crates/postui/src/main.rs crates/postui/src/app.rs
git commit -m "feat: seed theme from terminal colors via OSC query with fallback"
```

---

### Task 3: Paint layer core — surfaces, bevels, Button

**Files:**
- Create: `crates/postui/src/paint/mod.rs`
- Create: `crates/postui/src/paint/button.rs`
- Modify: `crates/postui/src/lib.rs` (add `pub mod paint;`)

**Interfaces:**
- Consumes: `Theme` tokens (Task 1).
- Produces:

```rust
// paint/mod.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlState { Normal, Hover, Pressed, Focused, Disabled }

pub fn fill(buf: &mut Buffer, area: Rect, bg: Color);                 // paints " " cells
pub fn bevel_top(buf: &mut Buffer, row: Rect, fg: Color, bg: Color);  // "▔" run
pub fn bevel_bottom(buf: &mut Buffer, row: Rect, fg: Color, bg: Color); // "▁" run
pub fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, fg: Color, bg: Color, bold: bool);

// paint/button.rs
pub enum ButtonKind { Primary, Secondary }
pub struct Button<'a> { pub label: &'a str, pub kind: ButtonKind, pub state: ControlState }
impl Button<'_> {
    /// area is exactly 3 rows tall; label is centered on the middle row.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme);
}
pub const BUTTON_HEIGHT: u16 = 3;
pub fn button_min_width(label: &str) -> u16;   // label + 4 cols padding
```

State mapping (Primary): Normal face=accent; Hover face=accent_edge_light-shifted one step (use `theme.tint(accent, control_hover)`? No — face stays `accent`, hover face = lift is already baked as `accent_edge_light`… keep simple and EXACT: Hover face = `theme.accent_edge_light`; Pressed face = `theme.accent_edge_dark` with bevel glyphs swapped (`▁` on top in accent_edge_dark-of-face, `▔` below); Disabled face = `theme.control`, label `text_disabled`, no bevel. Secondary: face `control` / hover `control_hover` / pressed `control_pressed`; edges `edge_light`/`edge_dark`. Focused = Normal + the label row's first and last cell get `focus_ring` colored `▎`/`▕`? No — Focused on buttons = Normal face with bevel rows drawn in `focus_ring`. Label: bold, `on_accent` (Primary) or `text` (Secondary).

- [ ] **Step 1: Write the failing tests** (in `paint/button.rs`)

```rust
use ratatui::{backend::TestBackend, Terminal};

fn buf_cell(term: &Terminal<TestBackend>, x: u16, y: u16) -> &ratatui::buffer::Cell {
    term.backend().buffer().cell((x, y)).unwrap()
}

#[test]
fn primary_button_paints_fill_bevel_and_centered_bold_label() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 1, 20, 3);
        Button { label: "Send", kind: ButtonKind::Primary, state: ControlState::Normal }
            .paint(f.buffer_mut(), area, &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 0, 1).symbol(), "▔");
    assert_eq!(buf_cell(&term, 0, 3).symbol(), "▁");
    let mid = buf_cell(&term, 8, 2); // "Send" centered in 20 cols starts at 8
    assert_eq!(mid.symbol(), "S");
    assert_eq!(mid.bg, theme.accent);
    assert_eq!(mid.fg, theme.on_accent);
    assert!(mid.modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn pressed_button_inverts_bevel() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
    term.draw(|f| {
        Button { label: "Send", kind: ButtonKind::Primary, state: ControlState::Pressed }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 0, 0).symbol(), "▁");
    assert_eq!(buf_cell(&term, 0, 2).symbol(), "▔");
}

#[test]
fn disabled_button_has_muted_label_and_no_bevel() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 3)).unwrap();
    term.draw(|f| {
        Button { label: "Send", kind: ButtonKind::Secondary, state: ControlState::Disabled }
            .paint(f.buffer_mut(), Rect::new(0, 0, 20, 3), &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 0, 0).symbol(), " ");
    assert_eq!(buf_cell(&term, 8, 1).fg, theme.text_disabled);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p postui paint::`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `paint/mod.rs` helpers and `Button::paint`**

`fill` iterates `area` positions setting symbol " " and bg. `bevel_top`/`bevel_bottom` set each cell in a 1-row rect to the glyph with fg/bg. `Button::paint` = fill 3 rows with face color per state table, bevel rows (skip when Disabled; `focus_ring` color when Focused), centered bold label on middle row.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p postui paint::` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src/paint crates/postui/src/lib.rs
git commit -m "feat: paint layer core with painted Button"
```

---

### Task 4: Paint layer — TextField, focus ring, Select, Chip, Tabs

**Files:**
- Create: `crates/postui/src/paint/field.rs` (TextField + focus_ring)
- Create: `crates/postui/src/paint/chip.rs` (Chip + Tabs)
- Modify: `crates/postui/src/paint/mod.rs` (re-exports)

**Interfaces:**
- Consumes: Task 3 helpers (`fill`, `bevel_*`, `text`, `ControlState`).
- Produces:

```rust
// paint/field.rs
pub struct TextField<'a> { pub content: ratatui::text::Line<'a>, pub state: ControlState }
impl TextField<'_> {
    /// area ≥ 3 rows; content drawn on middle row with 2-col left padding.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, theme: &Theme);
}
/// Accent ring in the CELLS SURROUNDING `inner` (one cell out on all sides),
/// drawn over `surround_bg`. Used by TextField (state == Focused) and by any
/// standalone focused control.
pub fn focus_ring(buf: &mut Buffer, inner: Rect, surround_bg: Color, theme: &Theme);
pub const FIELD_HEIGHT: u16 = 3;

// paint/chip.rs
pub struct Chip<'a> { pub label: &'a str, pub color: Color }  // method/status/count chips
impl Chip<'_> {
    /// 1 row; paints " label " with tinted bg (theme.tint(color, on)) and bold colored text.
    pub fn paint(&self, buf: &mut Buffer, x: u16, y: u16, on: Color, theme: &Theme) -> u16; // returns width painted
}
pub struct TabStrip<'a> { pub tabs: &'a [(String, bool)], pub active: usize }  // (label, has_badge)
impl TabStrip<'_> {
    /// 2 rows: labels row + underline row under the active tab.
    /// Returns the x-extent of each tab for hit registration.
    pub fn paint(&self, buf: &mut Buffer, area: Rect, on: Color, theme: &Theme) -> Vec<Rect>;
}
```

TextField states: Normal face `control` + bevels; Hover face `control_hover`; Focused = Normal + `focus_ring(inner_area, surrounding_bg)` — the caller passes an `area` already inset by 1 on all sides from the space it reserved, so the ring has room. Select is NOT a separate type: it is a `TextField` whose content line ends with a right-aligned `▾` in `text_muted` — provide `pub fn select_line(label: &str, width: u16, theme: &Theme) -> Line<'static>` in `field.rs` building that padded line.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn focused_field_draws_ring_in_surrounding_cells() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(30, 7)).unwrap();
    term.draw(|f| {
        let inner = Rect::new(2, 2, 26, 3);
        TextField { content: Line::raw("hello"), state: ControlState::Focused }
            .paint(f.buffer_mut(), inner, &theme);
        focus_ring(f.buffer_mut(), inner, theme.panel, &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 1, 1).symbol(), "┌");
    assert_eq!(buf_cell(&term, 1, 1).fg, theme.focus_ring);
    assert_eq!(buf_cell(&term, 28, 5).symbol(), "┘");
    assert_eq!(buf_cell(&term, 4, 3).symbol(), "h"); // 2-col padding
}

#[test]
fn chip_paints_tinted_pill() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(12, 1)).unwrap();
    term.draw(|f| {
        Chip { label: "GET", color: theme.success }
            .paint(f.buffer_mut(), 0, 0, theme.panel, &theme);
    }).unwrap();
    let c = buf_cell(&term, 1, 0);
    assert_eq!(c.symbol(), "G");
    assert_eq!(c.bg, theme.tint(theme.success, theme.panel));
    assert!(c.modifier.contains(Modifier::BOLD));
}

#[test]
fn tabstrip_underlines_active_tab_only() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
    let rects = /* paint TabStrip { tabs: &[("Params".into(), false), ("Headers".into(), false)], active: 0 } on Rect::new(0,0,40,2) */;
    // row 1 under "Params" is "▁" in accent; under "Headers" it is blank
    assert_eq!(buf_cell(&term, 1, 1).symbol(), "▁");
    assert_eq!(buf_cell(&term, 1, 1).fg, theme.accent);
    assert_eq!(buf_cell(&term, rects[1].x + 1, 1).symbol(), " ");
}
```

- [ ] **Step 2: Run tests to verify they fail** — `cargo test -p postui paint::` → FAIL.

- [ ] **Step 3: Implement field.rs and chip.rs** per the interface block.

- [ ] **Step 4: Run tests** — `cargo test -p postui paint::` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/postui/src/paint
git commit -m "feat: painted TextField, focus ring, chips, tab strip"
```

---

### Task 5: Paint layer — padded pill rows, table shell, floating panel

**Files:**
- Create: `crates/postui/src/paint/rows.rs` (pill rows)
- Create: `crates/postui/src/paint/panel.rs` (floating panel shell: backdrop dim + shadow)
- Modify: `crates/postui/src/paint/mod.rs` (re-exports)

**Interfaces:**
- Consumes: Tasks 3–4.
- Produces:

```rust
// paint/rows.rs
pub enum RowHighlight { None, Hover, Selected }
/// One logical list row on a 2-line pitch. `text_row` is the line holding
/// content; the half-row pads live in `text_row - 1` / `text_row + 1`
/// (drawn only when inside `bounds`). `base` is the surface behind the list.
pub struct PillRow { pub highlight: RowHighlight }
impl PillRow {
    pub fn paint(&self, buf: &mut Buffer, text_row: u16, x: u16, width: u16,
                 bounds: Rect, base: Color, theme: &Theme);
}
```

Fills: Hover → `control`; Selected → `control_hover` plus a 1-col accent bar at `x` (`█` on the text row, `▄`/`▀` caps in the pads). Pads: `▄` at `text_row-1` / `▀` at `text_row+1`, fg = this pill's fill, bg = whatever bg that cell already has — read the existing cell bg first, so two adjacent pills compose (upper pill's `▀` over lower pill's fill) exactly as the spec's shared-spacing-line rule requires. `RowHighlight::None` paints nothing.

```rust
// paint/panel.rs
/// Dim every cell in `area` (fg and bg blended 55% toward black).
pub fn dim_backdrop(buf: &mut Buffer, area: Rect);
/// Fill `area` with theme.panel and darken a 1-cell band right of and below it.
pub fn floating_panel(buf: &mut Buffer, area: Rect, screen: Rect, theme: &Theme);
```

`dim_backdrop` needs cell color math on `Color::Rgb`/`Color::Indexed`: for Rgb blend toward black; for Indexed, map through `rgb_to_indexed` after blending the xterm-256 nominal rgb (add `pub fn indexed_to_rgb(u8) -> (u8,u8,u8)` in `theme/mod.rs`, the inverse table of `rgb_to_indexed`'s cube/gray math).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn selected_pill_extends_half_rows_and_bar() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
    term.draw(|f| {
        paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
        PillRow { highlight: RowHighlight::Selected }
            .paint(f.buffer_mut(), 2, 0, 20, Rect::new(0, 0, 20, 5), theme.panel, &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 5, 1).symbol(), "▄");
    assert_eq!(buf_cell(&term, 5, 1).fg, theme.control_hover);
    assert_eq!(buf_cell(&term, 5, 2).bg, theme.control_hover);
    assert_eq!(buf_cell(&term, 0, 2).symbol(), "█");     // accent bar, full block on text row
    assert_eq!(buf_cell(&term, 0, 2).fg, theme.accent);
    assert_eq!(buf_cell(&term, 5, 3).symbol(), "▀");
}

#[test]
fn adjacent_pills_share_spacing_line() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 5)).unwrap();
    term.draw(|f| {
        paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 5), theme.panel);
        // selected at text_row 1, hovered at text_row 3 → they share row 2
        PillRow { highlight: RowHighlight::Hover }
            .paint(f.buffer_mut(), 3, 0, 20, Rect::new(0, 0, 20, 5), theme.panel, &theme);
        PillRow { highlight: RowHighlight::Selected }
            .paint(f.buffer_mut(), 1, 0, 20, Rect::new(0, 0, 20, 5), theme.panel, &theme);
    }).unwrap();
    let shared = buf_cell(&term, 5, 2);
    assert_eq!(shared.symbol(), "▀");                 // selected pill's bottom cap …
    assert_eq!(shared.fg, theme.control_hover);       // … in selection fill …
    assert_eq!(shared.bg, theme.control);             // … over the hover pill's fill
}

#[test]
fn floating_panel_darkens_shadow_band() {
    let theme = Theme::dark();
    let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
    term.draw(|f| {
        paint::fill(f.buffer_mut(), Rect::new(0, 0, 20, 10), theme.page);
        floating_panel(f.buffer_mut(), Rect::new(2, 2, 10, 5), Rect::new(0, 0, 20, 10), &theme);
    }).unwrap();
    assert_eq!(buf_cell(&term, 5, 4).bg, theme.panel);           // panel fill
    let shadow = buf_cell(&term, 12, 3).bg;                      // band right of panel
    let page = theme.page;
    assert_ne!(shadow, page);                                    // darkened
}
```

- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui paint::`.
- [ ] **Step 3: Implement rows.rs, panel.rs, `indexed_to_rgb`** per interface block.
- [ ] **Step 4: Run** — `cargo test -p postui` → PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/postui/src/paint crates/postui/src/theme
git commit -m "feat: pill rows with half-block padding, floating panel shell"
```

---

### Task 6: App bar reskin

**Files:**
- Modify: `crates/postui/src/components/header_bar.rs` (whole draw body)
- Modify: `crates/postui/src/ui.rs` (app bar area is 3 rows tall)
- Test: existing tests in `header_bar.rs` updated in place

**Interfaces:**
- Consumes: `fill`, `text`, Chip, `select_line`, `ControlState` (Tasks 3–4).
- Produces: `draw_header(frame, area /* 3 rows */, …existing args…)` — signature unchanged apart from the taller area.

- [ ] **Step 1: Update the component tests to assert the new idiom** — replace assertions about reversed-video title with: row 1 contains bold `postui` on `theme.panel` bg; the project chip cell bg is `theme.control`; project/env hits still registered at their (new) rects; right side shows the storage path in `text_muted`.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui header_bar`.
- [ ] **Step 3: Reimplement `draw_header`** — `fill(panel)` across all 3 rows; wordmark bold at x=3 row mid; project + env as 1-row `control`-filled chips with trailing `▾` (hover per `ctx.hovered` → `control_hover` bg); path right-aligned muted. Register the same `Hit::Project`/`Hit::Env` variants on the chip rects.
- [ ] **Step 4: Run `cargo test -p postui`** → PASS; adjust `ui.rs` layout constant (header height 1 → 3).
- [ ] **Step 5: Visual check** (recipe above): app bar matches mock; **commit** `git commit -am "feat: painted app bar"`.

---

### Task 7: Sidebar reskin — button, pill rows, 2-line pitch

**Files:**
- Modify: `crates/postui/src/components/sidebar.rs`
- Test: existing sidebar tests updated in place (e.g. `hovered_row_gets_background_not_inverted_text`, the `+ New request` assertions at lines ~744/776)

**Interfaces:**
- Consumes: Button, Chip, PillRow, `fill` (Tasks 3–5).
- Produces: unchanged `Component::draw` contract; row hit rects move to the 2-line pitch (`Hit::SidebarRow(i)` covers the text row AND its two half-pad rows so clicks in the pad select the row).

- [ ] **Step 1: Update tests** — `+ New request` renders as a 3-row Button (assert `▔` row, bold centered label on accent bg, no `[`/`]` anywhere in the pane); selected row asserts `control_hover` bg + `█` accent bar at column 0 of the text row; hovered row asserts `control` bg; rows are 2 lines apart (row i text at `y = top + 2*i`); method chips assert tinted bg via `theme.tint`.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui sidebar`.
- [ ] **Step 3: Reimplement draw** — `fill(panel)` whole pane; "REQUESTS" muted bold; Button (Primary, hover/pressed from `ctx.hovered` + existing press tracking); tree rows via PillRow + Chip + name text (selected bold); folder rows `⌄`/`›` muted; scrollbar spec math updated for 2-line pitch (`content = rows * 2`).
- [ ] **Step 4: Run full suite** → PASS (fix any stage-1..4 acceptance tests asserting bracket text — update them to the new assertions, same behaviors).
- [ ] **Step 5: Visual check** against mock sidebar; **commit** `git commit -am "feat: painted sidebar with pill rows"`.

---

### Task 8: Address bar — fused method/URL/Send signature control

**Files:**
- Modify: `crates/postui/src/components/editor.rs` (top strip)
- Modify: `crates/postui/src/app.rs` (Send press/pulse tick state if not already covered by `in_flight`)
- Test: editor tests updated in place

**Interfaces:**
- Consumes: `fill`, `bevel_*`, `text`, `select_line`, Button state mapping, `Theme::method_color`.
- Produces: one 3-row strip at the editor top; segments and their `Hit`s:
  - method segment (width 10): fill `method_color(method)`, bold `on_accent` label `"GET ▾"`, its own bevel from `tint`-lifted variants — compute per-segment edge colors with `theme.tint(method_color, theme.edge_light)`? NO — exact rule: edges for a colored face are `lift(face, ±0.12)`; expose `pub fn face_edges(face: Color, theme: &Theme) -> (Color, Color)` in `paint/mod.rs` (implemented here) so any colored segment gets correct bevels.
  - URL segment: TextField surface (`control`), flat-side-joined (no gap columns), existing `line_input` content line drawn on the middle row; focused URL → focus ring around the WHOLE fused bar.
  - Send cap (width 24): Primary Button visuals inline (not the Button type — the cap shares the bar's 3 rows); disabled while `in_flight.is_some()` is false and URL empty (existing enablement rule), and while in flight shows `⠋`-cycle spinner + "Sending" with face pulsing between `accent` and `accent_edge_dark` on the existing Tick action.
- `Hit` variants unchanged (`Hit::Method`, `Hit::Url`, `Hit::Send` — reuse the stage-4 names found in `hit.rs`).

- [ ] **Step 1: Update tests** — assert: bar occupies 3 rows; method cell bg == `theme.method_color(GET)`; URL text row shares y with method label; Send label bold on accent; while `sending == true` the label starts with a spinner glyph and Send hit is unregistered; focused URL draws `┌` at the bar's top-left minus 1.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui editor`.
- [ ] **Step 3: Implement** — add `face_edges` to `paint/mod.rs` (with its own unit test: edges of a face straddle its L), rebuild the top strip painting the three segments; wire spinner frames off the existing Tick.
- [ ] **Step 4: Run full suite** → PASS.
- [ ] **Step 5: Visual check** (send a request against a `python3 -m http.server` window in the held tmux session to see the in-flight state); **commit** `git commit -am "feat: fused address bar with painted send"`.

---

### Task 9: Params/headers table — compact rows, expanding active row, collapse

**Files:**
- Modify: `crates/postui/src/components/table_editor.rs`
- Modify: `crates/postui/src/components/editor.rs` (tab strip: count chip + `⌄ hide` toggle)
- Modify: `crates/postui/src/app.rs` + `crates/postui/src/keys.rs` (collapse state + key)
- Modify: `crates/postui/src/hit.rs` (new `Hit::TableCollapse` variant)
- Test: table_editor + editor tests updated; new collapse tests

**Interfaces:**
- Consumes: TabStrip, Chip, `fill`, PillRow mechanics (inline — the active table row pill spans full row width), `▏` divider glyph.
- Produces:
  - Table layout fn: header row (muted uppercase NAME/VALUE on `panel`), body = one `control` surface; inactive rows 1 line; the ACTIVE row (cell being edited, or hovered row) occupies 3 lines (pad/text/pad) with `control_hover` full-row fill, accent `█` bar at row left (caps in pads), `▎` + cursor in the active cell, `✕` at right; ghost `+ Add param` last row (muted; hover `control_hover`); `▔` edge row in `edge_dark` below the block.
  - `pub fn table_height(rows: usize, active: Option<usize>) -> u16` = `1 + rows + active.map_or(0, |_| 2) + 1 + 1` (header + rows + expansion + ghost + edge) — `editor.rs` uses it for layout.
  - Collapse: `App.table_collapsed: bool` (persisted per session only); toggled by `Hit::TableCollapse` click or key `alt+p`; when collapsed the table body is skipped and the freed rows go to the response pane; tab strip always shows the active tab's count chip (`params.len()`).
- Cursor/editing logic, row add/delete, and `Hit::TableCell{row,col}` semantics are UNCHANGED — hit rects just move with the new geometry (active row's rect covers all 3 lines).

- [ ] **Step 1: Write/adjust the failing tests**

```rust
#[test]
fn active_row_expands_to_three_lines() { /* draw with editing row 1 of 3; assert text
    rows at y = body_top, body_top+2 (pad above active), and the active text row bg
    == theme.control_hover across full width; "▄" row above it; inactive rows 1-line. */ }

#[test]
fn collapse_hides_body_and_keeps_count_chip() { /* set table_collapsed = true; assert
    no NAME header row painted, response pane area grew by table_height, tab strip
    cell at the count-chip x has bg == theme.tint(theme.accent, theme.page) and "3". */ }

#[test]
fn collapse_toggle_click_and_key() { /* click Hit::TableCollapse → App.table_collapsed
    flips; key alt+p flips it back. */ }
```

Plus updates to every existing table_editor drawing assertion (bracket `a add` hints → ghost row text `+ Add param`).

- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui table_editor collapse`.
- [ ] **Step 3: Implement** table painting, `table_height`, collapse state/key/hit, tab-strip chip + `⌄ hide`/`› show` toggle (muted, right-aligned, hover → `text` color).
- [ ] **Step 4: Run full suite** → PASS.
- [ ] **Step 5: Visual check** against mock (compact rows + expanded editing row + chip + hide toggle); **commit** `git commit -am "feat: painted params table with expanding active row and collapse"`.

---

### Task 10: Response pane reskin

**Files:**
- Modify: `crates/postui/src/components/response.rs`
- Test: response tests updated in place

**Interfaces:**
- Consumes: Chip, TabStrip, `fill`, `Theme::status_color`.
- Produces: 3-row header strip on `panel` — status Chip (`"200 OK"` colored by `status_color`), timing + size as `control`-filled muted chips, response tabs right-aligned with accent underline; body on `page` (json_tree/raw drawing unchanged); empty state centered muted invitation (existing copy) on `page`.

- [ ] **Step 1: Update tests** — status chip bg == `theme.tint(theme.success, theme.panel)` for 200; strip is 3 rows on `panel`; tabs register the same hits; body area unchanged relative to new strip height; empty-state text still present.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui response`.
- [ ] **Step 3: Implement** the strip; keep scrollbar and json_tree calls as-is.
- [ ] **Step 4: Run full suite** → PASS.
- [ ] **Step 5: Visual check** (send request in tmux, compare chips row to mock); **commit** `git commit -am "feat: painted response header strip"`.

---

### Task 11: Floating shells — modal, chooser, var picker, palette

**Files:**
- Modify: `crates/postui/src/components/modal.rs`
- Modify: `crates/postui/src/components/chooser.rs`
- Modify: `crates/postui/src/components/var_picker.rs`
- Modify: `crates/postui/src/components/palette.rs`
- Test: each component's tests updated in place

**Interfaces:**
- Consumes: `dim_backdrop`, `floating_panel`, TextField + `focus_ring`, Button, PillRow, Chip.
- Produces: every overlay draws `dim_backdrop(whole screen)` then `floating_panel(rect)`; modal = bold title, muted helper line, TextField with ring, right-aligned Secondary Cancel + Primary confirm Buttons (hits unchanged); chooser/var_picker/palette = TextField search on top (ring when focused), PillRow results (selected = neutral pill + accent bar, matching the sidebar), muted hint footer line; palette keeps its keybinding column right-aligned muted. Modal backdrop click-to-dismiss / chrome-click rules from stage 4 are unchanged — only painting changes.

- [ ] **Step 1: Update tests** — modal: backdrop cell dimmed (bg != theme.page for a cell outside the panel), panel bg == `theme.panel`, confirm button bevel `▔` present, Cancel face `control`; palette: selected row `control_hover` + `█` bar; chooser rows 2-line pitch.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui modal chooser var_picker palette`.
- [ ] **Step 3: Implement** all four using the shared shells; delete each component's private border/Block chrome.
- [ ] **Step 4: Run full suite** → PASS (stage-4 modal chrome-click tests must still pass — the panel rect is the hit boundary, unchanged semantics).
- [ ] **Step 5: Visual check** against `mock-modal.png` / `mock-palette.png`; **commit** `git commit -am "feat: floating panel shells for modal, choosers, palette"`.

---

### Task 12: Footer toolbar, toasts, pane surfaces + legacy removal

**Files:**
- Modify: `crates/postui/src/components/footer.rs` (chip toolbar, 3 rows)
- Modify: `crates/postui/src/components/toast.rs` (filled panel + colored left bar)
- Modify: `crates/postui/src/ui.rs` (pane backgrounds: sidebar `panel`, editors `page`, 1-col painted gutter; drop `│` separators)
- Modify: `crates/postui/src/components/mod.rs` (DELETE `pane_block`)
- Modify: `crates/postui/src/hit.rs` (DELETE bracket `button`, `button_width`, legacy `chip`; keep HitMap/ScrollbarSpec)
- Modify: `crates/postui/src/theme/mod.rs` (DELETE legacy alias fields `surface`, `surface_raised`, `border`, `border_focused`; fix all remaining references to use new tokens)
- Modify: `crates/postui/src/components/editor.rs`, `response.rs`, `json_tree.rs`, `line_input.rs` — wherever legacy fields/`pane_block` were still referenced
- Test: footer/toast tests updated; `cargo build` proves no legacy references remain

**Interfaces:**
- Consumes: everything prior.
- Produces: the finished stage-5 surface; no `Borders::ALL` blocks, no `[`-bracket buttons, no legacy theme fields anywhere in `crates/postui`.

- [ ] **Step 1: Update tests** — footer: 3-row `panel` toolbar, each binding a `control`-filled chip with accent bold key + muted label, `q quit` right muted; toast: `panel` fill + `█` left bar in success/error color; ui: cell in gutter column has `page` bg, sidebar cell `panel` bg, NO `│` glyph at the gutter x.
- [ ] **Step 2: Run to verify FAIL** — `cargo test -p postui footer toast ui`.
- [ ] **Step 3: Implement** footer/toast/ui; then delete `pane_block`, `hit::button`/`button_width`/`chip`, and the four legacy theme fields; chase every compile error to the new tokens (`surface`→`page`, `surface_raised`→`panel`, `border`→`edge_light`, `border_focused`→`focus_ring`).
- [ ] **Step 4: Run the FULL suite including acceptance tests** — `cargo test -p postui && cargo test -p postui-core`. Expected: PASS; `grep -rn "pane_block\|\[ " crates/postui/src --include=*.rs | grep -v test` finds no bracket-button remnants.
- [ ] **Step 5: Full visual sweep** — tmux: main screen, open modal (`r` rename), palette (`^P`), send a request; render all four captures to PNG, compare to the three approved mocks, and SEND the PNGs to the user for final visual sign-off. **Commit** `git commit -am "feat: painted footer, toasts, pane surfaces; remove legacy chrome"`.

---

## Self-Review

- **Spec coverage:** theme engine (T1), OSC terminal fit + config (T2), button/bevel/states (T3), field/ring/select/chips/tabs (T4), pill rows + shared spacing + floating shell (T5), app bar (T6), sidebar (T7), signature address bar + in-flight pulse (T8), compact-table + expanding active row + collapse-on-tab-strip (T9), response chips (T10), overlays (T11), footer/toasts/surfaces/gutter + cohesion enforcement by deletion (T12). 256-downgrade covered in T1; interaction-state table is encoded in Button/TextField state mappings (T3/T4); disabled-unregistered rule kept in T8 (Send while in flight).
- **Placeholder scan:** none of the banned patterns; T9 step-1 tests are abbreviated by design but state exact assertions to write.
- **Type consistency:** `ControlState` (T3) used by T4/T8; `PillRow`/`RowHighlight` (T5) used by T7/T11; `face_edges` defined T8, used only there and T8's own test; `status_color`/`tint` defined T1, used T10/T9/T4; `table_height` defined T9 and consumed in the same task's editor.rs change.
