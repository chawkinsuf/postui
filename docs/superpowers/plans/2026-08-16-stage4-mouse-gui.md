# Stage 4: Mouse-First GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make postui feel like a GUI for mouse users — hit-testing, hover, in-pane buttons, method dropdown, draggable scrollbars, clipboard copy, palette frecency — while the keyboard remains a complete alternative.

**Architecture:** A `HitMap` rebuilt during every render: each clickable widget registers `(Rect, Hit)` as it draws (last-registered wins, so things drawn on top win). Mouse events resolve by point lookup and dispatch through the existing `Action` enum — the same code paths keyboard bindings use. Hover is a lookup on motion events with redraw only when the hovered `Hit` changes. Drag (scrollbars) is a small state machine keyed off the `Hit` captured on mouse-down.

**Tech Stack:** Rust, ratatui 0.29 (crossterm via `ratatui::crossterm`), edtui, tokio, arboard + base64 (new deps), wiremock (tests).

**Spec:** `docs/superpowers/specs/2026-08-16-stage4-mouse-gui-design.md` — binding authority. Read it before starting your task.

## Global Constraints

- Cargo is not on the sandbox PATH. Prefix every cargo command: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Gate for every task: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --all` applied before commit.
- Import crossterm types via `ratatui::crossterm::...` only; never add a mismatched direct crossterm dep (verify `cargo tree -i crossterm` shows exactly one version if you touch deps).
- No functionality may require vim-style keys or modes. Every mouse affordance added in this stage must keep (or gain) a keyboard path — usually an existing keybinding or a palette command (spec §9).
- Interface writing: buttons named for what they do; one verb per flow (`Send` → "Sending…" → "Sent"; `Copy` → "Copied …"); errors state the fix; empty states invite action.
- Pinned values (spec): double-click window **400 ms**; OSC 52 threshold default **65536** bytes (`osc52_limit` config key); scrollbar track click **pages** (±viewport); save-body prefill `~/Downloads/<slug>-response.<ext>`.
- Exact toast strings (spec §7): over-threshold OSC 52 → `Too large for the terminal clipboard — use Save body to file, or set clipboard_cmd in config`; all tiers failed → `Clipboard unavailable — try Shift+drag to select`.
- Commit messages: imperative, no Co-Authored-By, no Claude-Session trailer.

### tmux visual verification (required for every task that changes rendering)

The sandbox kills the tmux server when a Bash call exits, so hold it with a background call:

```bash
# background (run_in_background: true) holder call:
export TMUX_TMPDIR=/tmp/claude-1000/tmux
tmux kill-server 2>/dev/null
export XDG_CONFIG_HOME=/tmp/claude-1000/postui-config   # keep real ~/.config untouched
tmux new-session -d -s postui -x 160 -y 45 "$PWD/target/debug/postui" && sleep 3600
```

Then in normal calls (always `export TMUX_TMPDIR=/tmp/claude-1000/tmux` first):
- keys: `tmux send-keys -t postui:0 'n'`, `M-u`, `C-r`, `Tab`, `Enter`, `Escape`; sleep ~0.4s after sends
- screen: `tmux capture-pane -t postui:0 -p` (add `-e` to see colors/styles)
- mouse (SGR bytes, 1-based coords): press+release left click at COL,ROW =
  `printf '\x1b[<0;COL;ROWM\x1b[<0;COL;ROWm'` sent via `tmux send-keys -t postui:0 -H <hex bytes>`
  (button 0 = left; 64/65 = wheel up/down; motion events = button 35 with `M` only)
- hover check: send a motion sequence `\x1b[<35;COL;ROWM`, capture with `-e`, confirm the styling changed
- done: send `q`, `tmux kill-server`, TaskStop the holder

Look at the capture yourself and fix anything unpolished (misaligned buttons, clipped popups, wrong hover styling) before closing the task. Focus is invisible in captures — read the footer hints to know the focused pane.

## File structure (end state)

- Create `crates/postui/src/hit.rs` — `Hit`, `HitMap`, button/chip render helpers, scrollbar helper.
- Create `crates/postui/src/clipboard.rs` — tiered clipboard (`clipboard_cmd` → arboard → OSC 52 + threshold).
- Create `crates/postui/src/usage.rs` — palette usage store (frecency), persisted to `~/.config/postui/ui.toml`.
- Modify `crates/postui/src/{ui.rs,app.rs,main.rs,action.rs,config.rs,lib.rs}` and `components/{mod.rs,header_bar.rs,footer.rs,sidebar.rs,editor.rs,response.rs,modal.rs,chooser.rs,palette.rs,var_picker.rs,table_editor.rs,json_tree.rs}`.
- Modify `crates/postui-core/src/prepare.rs` (carryover radar fixes only).

---

### Task 1: Hit-testing module

**Files:**
- Create: `crates/postui/src/hit.rs`
- Modify: `crates/postui/src/lib.rs` (add `pub mod hit;`)

**Interfaces:**
- Produces (later tasks depend on these exact names):

```rust
use crate::layout::PaneId;
use ratatui::layout::Rect;

/// A typed clickable target. Registered with its screen Rect during render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// Pane background: click focuses, wheel scrolls.
    Pane(PaneId),
    HeaderProject,
    HeaderEnv,
    /// A footer hint chip that dispatches its action on click.
    FooterChip(crate::action::Action),
    SidebarNewRequest,
    /// Index into `sidebar.rows`.
    SidebarRow(usize),
    SidebarFolderArrow(usize),
    /// Renders as Cancel while a request is in flight.
    SendButton,
    CopyUrlButton,
    MethodSelector,
    /// 0 = Params, 1 = Headers, 2 = Body.
    EditorTab(usize),
    TableRow(usize),
    TableCheckbox(usize),
    /// Raw mouse event forwarded to edtui (click-to-place, wheel).
    BodyEditor,
    ResponseTab(crate::components::response::ViewMode),
    CopyBodyButton,
    SaveBodyButton,
    /// Copy icon on row `i` of the response Headers view.
    HeaderCopy(usize),
    /// Visible row `i` of the JSON tree (click selects).
    JsonRow(usize),
    /// The ▸/▾ glyph cell of visible row `i` (click toggles).
    JsonArrow(usize),
    ScrollbarThumb(PaneId),
    /// Signed page delta applied on click (±viewport height).
    ScrollbarTrack(PaneId, i16),
    DropdownRow(usize),
    ChooserRow(usize),
    PaletteRow(usize),
    VarPickerRow(usize),
    /// A clickable `[y] Label` chip in a Confirm modal.
    ConfirmChoice(char),
    /// Full-screen region under an open modal; click closes (same as Esc).
    ModalOutside,
}

#[derive(Default)]
pub struct HitMap { regions: Vec<(Rect, Hit)> }

impl HitMap {
    pub fn clear(&mut self);
    pub fn register(&mut self, rect: Rect, hit: Hit);
    /// Topmost (= last registered) hit containing the point.
    pub fn hit_at(&self, x: u16, y: u16) -> Option<&Hit>;
    /// Topmost `Hit::Pane` containing the point (for wheel routing).
    pub fn pane_at(&self, x: u16, y: u16) -> Option<PaneId>;
    /// Last-registered rect for `hit` — the test helper click tests use.
    pub fn rect_of(&self, hit: &Hit) -> Option<Rect>;
}

/// Draws a bracketed button `[ label ]` and registers it (only when enabled).
/// Styling: accent fg at rest; inverted (accent bg, surface fg) when
/// `hovered == Some(&hit)`; text_muted and unregistered when disabled.
pub fn button(
    frame: &mut ratatui::Frame, hits: &mut HitMap, area: Rect, label: &str,
    hit: Hit, hovered: Option<&Hit>, enabled: bool, theme: &crate::theme::Theme,
);
/// Same styling contract for a plain (unbracketed) clickable chip/label.
pub fn chip(
    frame: &mut ratatui::Frame, hits: &mut HitMap, area: Rect, label: &str,
    hit: Hit, hovered: Option<&Hit>, theme: &crate::theme::Theme,
);
/// `[ label ]` rendered width, for layout math.
pub fn button_width(label: &str) -> u16;
```

- [ ] **Step 1: Write failing tests** in `hit.rs` `#[cfg(test)]`:

```rust
#[test]
fn last_registered_hit_wins_at_a_point() {
    let mut m = HitMap::default();
    m.register(Rect::new(0, 0, 10, 10), Hit::Pane(PaneId::Sidebar));
    m.register(Rect::new(2, 2, 3, 1), Hit::SidebarRow(0));
    assert_eq!(m.hit_at(3, 2), Some(&Hit::SidebarRow(0)));
    assert_eq!(m.hit_at(0, 0), Some(&Hit::Pane(PaneId::Sidebar)));
    assert_eq!(m.hit_at(50, 50), None);
    assert_eq!(m.pane_at(3, 2), Some(PaneId::Sidebar), "pane_at sees through overlays");
    assert_eq!(m.rect_of(&Hit::SidebarRow(0)), Some(Rect::new(2, 2, 3, 1)));
}

#[test]
fn button_renders_and_registers_only_when_enabled() {
    // Render into a TestBackend; assert "[ Send ]" text present, hit registered.
    // Re-render with enabled=false; assert not registered and muted styling.
    // Re-render with hovered=Some(&Hit::SendButton); assert bg == theme.accent
    // on a cell inside the button (inspect buffer cell styles).
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p postui hit::` fails (module missing).
- [ ] **Step 3: Implement** — `regions.iter().rev().find(...)` for `hit_at`/`rect_of`; `pane_at` filters `Hit::Pane`. `button` renders `[ {label} ]` as a `Paragraph` with the styling contract above and calls `hits.register` when enabled.
- [ ] **Step 4: Tests pass**, clippy, fmt.
- [ ] **Step 5: Commit** — `feat: hit-testing module (HitMap, Hit, button helpers)`

Note: `Hit` references `response::ViewMode` — derive `Eq` on `ViewMode` already exists; also add `#[derive(Debug, Clone, PartialEq, Eq)]` on `Action`? `Action` already derives those; do not change it.

---

### Task 2: Thread the HitMap through rendering and mouse routing

**Files:**
- Modify: `crates/postui/src/components/mod.rs` (DrawCtx + Component::draw signature)
- Modify: `crates/postui/src/ui.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`
- Modify: every `Component::draw` impl and modal/palette/chooser/var_picker/header/footer draw fn (mechanical signature change)

**Interfaces:**
- `DrawCtx` becomes:

```rust
pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub focused: bool,
    pub hovered: Option<&'a crate::hit::Hit>,
}
pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> { None }
    fn handle_scroll(&mut self, _delta: i16) {}
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, hits: &mut crate::hit::HitMap);
}
```

- `App` gains fields (all `pub` unless noted): `hits: HitMap`, `hovered: Option<Hit>`, `drag: Option<Drag>` (used in Task 10; define `pub struct Drag { pub pane: PaneId, pub grab_offset: u16 }` in app.rs now), private `last_click: Option<(Hit, std::time::Instant)>`.
- `App::handle_mouse(&mut self, m: MouseEvent) -> bool` — **layout parameter removed**. Behavior:
  1. `Moved`: if a drag is active, delegate (Task 10; until then ignore). Else resolve `hit_at`; if it differs from `self.hovered`, store and return true; else false.
  2. `Down(Left)`: resolve hit (clone it); compute `clicks` (2 if same hit within 400 ms of `last_click`, else 1; update `last_click`); call `self.on_hit(hit, clicks, m)`.
  3. `Up(Left)`: clear `self.drag`; return false if nothing changed.
  4. `ScrollUp/ScrollDown`: if `hit_at` is `Hit::BodyEditor` and `self.editor.handle_mouse(m)` consumes → `update(Action::Render)`. Else `pane_at` → `update(Action::ScrollPane(pane, ±3))`. (When a modal is open `pane_at` still finds panes — guard: if `!self.modals.is_empty()`, wheel is a no-op until Task 11 adds modal-list scrolling.)
- `fn on_hit(&mut self, hit: Hit, clicks: u8, m: MouseEvent) -> bool` — the central mapping; this task implements only:
  - `Hit::Pane(p)` → `update(Action::FocusPane(p))`
  - `Hit::BodyEditor` → focus editor pane, forward `m` to `self.editor.handle_mouse(m)` (preserves click-to-place-cursor), `update(Action::Render)`
  - everything else → `false` for now (later tasks extend this match; leave a `_ => false` arm)
- `ui::draw(frame, app)` — take `app.hits` out (`std::mem::take`), `clear()`, register the three pane rects (`Hit::Pane(...)`) **first**, then pass `&mut hits` down every draw call, then put it back. Build each pane's `DrawCtx` with `hovered: app.hovered.as_ref()`.
- `main.rs`: mouse arm becomes unconditional and layout-free:

```rust
Event::Mouse(m) => { redraw |= app.handle_mouse(m); }
```

(the modal-open gate is removed — modal draw order will make modal hits win; until Task 11 registers them, clicks while a modal is open resolve to pane hits, so for THIS task keep a guard: on Down with `!self.modals.is_empty()`, return false. Task 11 removes it.)

- `layout::hit_test` becomes unused by the app — delete it and its test (the HitMap covers it).

- [ ] **Step 1: Write failing tests** (in `app.rs` tests):

```rust
#[test]
fn click_on_pane_hit_focuses_that_pane() {
    let mut app = App::new_for_test();
    render_once(&mut app); // TestBackend 120x40 draw, populates app.hits
    let r = app.hits.rect_of(&crate::hit::Hit::Pane(PaneId::Response)).unwrap();
    app.handle_mouse(left_down(r.x + 2, r.y + 2));
    assert_eq!(app.focus, PaneId::Response);
}

#[test]
fn hover_change_requests_redraw_and_same_hover_does_not() {
    let mut app = App::new_for_test();
    render_once(&mut app);
    let r = app.hits.rect_of(&crate::hit::Hit::Pane(PaneId::Sidebar)).unwrap();
    assert!(app.handle_mouse(moved(r.x + 1, r.y + 1)), "first hover redraws");
    assert!(!app.handle_mouse(moved(r.x + 1, r.y + 2)), "same hit: no redraw");
}

#[test]
fn body_click_to_place_cursor_still_works_via_hitmap() {
    // Port of the existing click_in_body_area_places_cursor_and_focuses_content
    // test, now going through app.handle_mouse(m) with no layout argument.
}
```

Add local helpers `fn left_down(x,y) -> MouseEvent`, `fn moved(x,y) -> MouseEvent` (`MouseEventKind::Moved`), and `fn render_once(app)` (TestBackend 120×40 + `ui::draw`).

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement.** The signature change is wide but mechanical — every existing `draw` gains `hits: &mut HitMap` (unused `_hits` in components not yet converted). The editor's Body arm must register `Hit::BodyEditor` over `last_body_area` (keep the field; it still gates `editor.handle_mouse`). Fix the two existing tests that call `app.handle_mouse(m, &layout)` to the new signature.
- [ ] **Step 4: Full test suite passes** (existing mouse tests included), clippy, fmt.
- [ ] **Step 5: tmux check** — build, run in tmux, click each pane border region and confirm focus follows (footer hints change); wheel still scrolls; motion sequences don't flood redraws (app stays responsive).
- [ ] **Step 6: Commit** — `feat: frame-built HitMap wired through render and mouse routing, hover state`

---

### Task 3: Header selectors and clickable footer

**Files:**
- Modify: `crates/postui/src/components/header_bar.rs`, `crates/postui/src/components/footer.rs`, `crates/postui/src/ui.rs`, `crates/postui/src/app.rs` (on_hit arms)

**Interfaces:**
- `draw_header(frame, area, theme, project, env, hits, hovered)` — project and env each render as `name ▾`; the hovered one gains `Modifier::UNDERLINED`. Registers `Hit::HeaderProject` over the project span's rect and `Hit::HeaderEnv` over the env span's rect (compute x offsets from the span widths).
- `draw_footer(frame, area, theme, focus, hits, hovered)` — restructured around `fn footer_chips(focus: PaneId) -> Vec<(String, Option<Action>)>`:
  - Sidebar: `("enter open", None)`, `("n new", Some(PromptNewRequest))`, `("r rename", Some(PromptRenameRequest))`, `("d delete", Some(ConfirmDeleteRequest))`
  - Editor: `("ctrl+r send", Some(Send))`, `("ctrl+s save", Some(SaveRequest))`, `("alt+1/2/3 tabs", None)`
  - Response: `("r raw", None)`, `("h headers", None)`, `("/ search", None)`
  - Always appended: `("^P commands", Some(OpenPalette))`, `("q quit", Some(Quit))`
  - Chips with `Some(action)` render via `hit::chip` with `Hit::FooterChip(action)`; `None` chips render muted, unregistered.
- `App::on_hit` arms: `HeaderProject → update(OpenProjectChooser)`, `HeaderEnv → update(OpenEnvChooser)`, `FooterChip(a) → update(a)`.

- [ ] **Step 1: Failing tests** — render app, `rect_of(&Hit::HeaderEnv)` exists; click it → env chooser toast (no envs in test project: expect the "no environments" warning toast, proving the action fired); click `Hit::FooterChip(Action::OpenPalette)` → palette modal open; header buffer contains `▾`.
- [ ] **Step 2: Verify failure. Step 3: Implement. Step 4: Suite green, clippy, fmt.**
- [ ] **Step 5: tmux check** — hover the env name (motion event at its cell): underline appears in `-e` capture; click opens the chooser; footer chips hover-invert; `⌘`-style palette chip opens palette by click.
- [ ] **Step 6: Commit** — `feat: clickable header project/env selectors and footer chips`

---

### Task 4: Sidebar — clickable rows, folder arrows, New request button

**Files:**
- Modify: `crates/postui/src/components/sidebar.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- Sidebar draw: inside the pane block, first line is a `[ + New request ]` button (via `hit::button`, `Hit::SidebarNewRequest`), then one blank spacer line, then the rows (list area shrinks by 2). The empty state keeps its text below the button.
- Each visible row registers `Hit::SidebarRow(i)` over its full line (i = index into `self.rows`). Folder rows additionally register `Hit::SidebarFolderArrow(i)` over the `▸`/`▾` glyph cell (registered after the row hit so the arrow wins).
- Hovered rows render with `Style.bg(theme.surface_raised)` (rows are not buttons; background hover, not inversion).
- `App::on_hit`:
  - `SidebarNewRequest` → `update(PromptNewRequest)`
  - `SidebarFolderArrow(i)` → focus sidebar, `sidebar.selected = i`, `update(ToggleSelectedFolder)`
  - `SidebarRow(i)` → focus sidebar, `sidebar.selected = i`; then match `rows[i]`: `Request{broken: None}` → `update(OpenRequest(slug))` (single click opens); `Request{broken: Some}` → `update(ShowRequestError(slug))`; `Folder` → select only on single click, `update(ToggleSelectedFolder)` when `clicks == 2`.

- [ ] **Step 1: Failing tests** — with a project containing `api/ping` + `top`:
  - click `SidebarRow` of `top` → `editor.slug == Some("top")`
  - click `SidebarFolderArrow` of the `api` folder → folder expands (rows lengthen)
  - single click folder name selects but does not expand; second click within 400 ms (call `on_hit` via two `handle_mouse` downs) expands
  - click `SidebarNewRequest` → `Modal::Prompt{kind: NewRequest}` on top
  - dirty-editor guard: dirty editor + click another request row → Confirm modal, not a silent open (reuses `OpenRequest`'s gate — assert it)
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — button aligned at top of pane, hover states on rows, arrow click vs name click behave differently, double-click timing feels right.
- [ ] **Step 6: Commit** — `feat: clickable sidebar rows, folder arrows, New request button`

---

### Task 5: Editor — clickable tabs, table cells, Send/Cancel button

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `crates/postui/src/components/table_editor.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- `Editor` gains `pub sending: bool` (synced in `App::update` after every action: `self.editor.sending = self.in_flight.is_some();` next to the existing `open_slug` sync) and `pub last_method_area: Option<Rect>` (recorded every draw; consumed by Task 6).
- URL row layout becomes `[method 8][url Min][copy-url reserved 0 this task][send button]`: right end renders `[ Send ]`, or `[ Cancel ]` when `sending` — **same width**: label padded to the wider of the two (`Cancel`), so nothing jumps. Registers `Hit::SendButton` either way. The method badge cell registers `Hit::MethodSelector` (action wired in Task 6).
- Tab bar renders each label via `hit::chip` with `Hit::EditorTab(i)` (keep the active-tab accent+bold styling; hover per the chip contract).
- `TableEditorState::draw` gains a `hits: &mut HitMap` + `row_area` knowledge: register per visible row `Hit::TableRow(i)` over the full line and `Hit::TableCheckbox(i)` over the `✓/✗` cell (after the row, so the checkbox wins). (Rows are not scrolled today; index = map index.)
- `App::on_hit`:
  - `EditorTab(i)` → focus editor, `update(EditorTabSelect(i))`
  - `SendButton` → `update(if self.in_flight.is_some() { CancelSend } else { Send })`
  - `TableCheckbox(i)` → focus editor, `sub_focus = Content`, `table.selected = i`, toggle `enabled` on the active tab's map entry `i`, true
  - `TableRow(i)` → focus editor, `sub_focus = Content`, `table.selected = i`; on `clicks == 2` begin editing the key cell (same as the table's Enter path — factor `TableEditorState::begin_edit_selected(&mut self, map)` out of the `KeyCode::Enter` arm and call it)
- `MethodSelector` → no-op this task (`false`), wired in Task 6.

- [ ] **Step 1: Failing tests:**
  - click `EditorTab(2)` → `active_tab == Body`
  - with a param row, click its `TableCheckbox(0)` → `enabled` flipped; click `TableRow(0)` twice within 400 ms → `table.editing.is_some()` with the key cell seeded
  - click `SendButton` with URL set → `in_flight.is_some()`; render again (button now reads Cancel — assert buffer contains `Cancel`); click again → `ResponseState::Cancelled`
  - Send/Cancel same width: assert `rect_of(&Hit::SendButton)` equal before and after send
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — Send button placement right of URL, hover inversion, in-flight swap to Cancel with no layout jump, tab clicks, checkbox clicks.
- [ ] **Step 6: Commit** — `feat: clickable editor tabs, table cells, Send/Cancel button`

---

### Task 6: Dropdown popup primitive + method selector

**Files:**
- Modify: `crates/postui/src/components/modal.rs`, `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/components/palette.rs`, `crates/postui/src/components/editor.rs` (method-cell hit already registered)

**Interfaces:**

```rust
// modal.rs
pub struct DropdownState {
    pub anchor: Rect,                     // the cell it opens from
    pub items: Vec<(String, Action)>,
    pub selected: usize,
}
Modal::Dropdown(DropdownState)
```

- Drawing: bordered list at `anchor.x, anchor.y + 1`, width = longest label + 4, height = items + 2; **flips upward** (`anchor.y - height`) when it would cross the screen bottom; clamped horizontally. Renders LAST like other modals but **skips `dim_backdrop`** (restructure `ModalStack::draw` so the dim call is per-variant: every variant except `Dropdown` dims). Registers `Hit::ModalOutside` over the whole screen first, then `Hit::DropdownRow(i)` per row (Task 11 wires ModalOutside for all modals; wire it for Dropdown NOW: on_hit `ModalOutside` → `update(Action::Close)` — it is harmless for other modals since their hits aren't registered yet and the Task-2 guard still blocks them).
- Keys (in `ModalStack::handle_key`): Up/Down move selection (clamped), Enter → `ModalResult { actions: vec![items[selected].1.clone()], close: true }`, Esc → close, everything else swallowed.
- New actions: `Action::OpenMethodDropdown`, `Action::SetMethod(postui_core::model::Method)`.
- `App::apply`:
  - `OpenMethodDropdown` → build items from all 7 methods (`Method::Get, Post, Put, Patch, Delete, Head, Options` — hardcode the array; label = `method.as_str()`, action = `SetMethod(m)`), `selected` = current method's index, anchor = `editor.last_method_area.unwrap_or_else(|| Rect::new(0,0,0,0))` (a zero anchor draws top-left — acceptable; it exists after any frame), push `Modal::Dropdown`.
  - `SetMethod(m)` → `self.editor.method = m; true`
- `App::on_hit`: `MethodSelector` → focus editor + `update(OpenMethodDropdown)`; `DropdownRow(i)` → dispatch that row's action and pop the modal (read the action via `modals.top_mut()`, clone, pop, update).
- Add `ModalStack::top_mut(&mut self) -> Option<&mut Modal>`.
- Palette: new command `Command { id: "method-choose", name: "Method: choose…", action: OpenMethodDropdown }` next to "Method: cycle". (`id` field arrives in Task 12 — if this task lands first, add the command without `id` and let Task 12 add ids to all. Order in this plan has ids later, so: add name+action only.)
- Current method row shows a `✓` marker.

- [ ] **Step 1: Failing tests:**
  - `OpenMethodDropdown` pushes `Modal::Dropdown` with 7 items, selected == current method index
  - Down/Down/Enter → method changed to the third entry, modal closed
  - Esc closes without change; keys don't leak (send `q`, app must not quit)
  - click `MethodSelector` hit → dropdown opens; click `DropdownRow(3)` → `editor.method == Method::Patch`, modal closed
  - flip-up: with anchor near bottom (construct DropdownState directly, draw into small TestBackend), rendered rows sit above the anchor row
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — click GET badge, popup anchored below it, hover rows, pick POST, popup closes, badge updates; open near bottom via resized pane if feasible (or trust the unit test); existing `alt+m` cycle still works.
- [ ] **Step 6: Commit** — `feat: anchored dropdown popup; clickable method selector`

---

### Task 7: Clipboard module and UI settings

**Files:**
- Create: `crates/postui/src/clipboard.rs` (+ `pub mod clipboard;` in lib.rs)
- Modify: `crates/postui/src/config.rs`, `crates/postui/Cargo.toml` (add `arboard`, `base64`)

**Interfaces:**

```rust
// config.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettings {
    pub clipboard_cmd: Option<String>,
    pub osc52_limit: usize,          // default 65536
}
impl Default for UiSettings { /* None, 65536 */ }
/// Reads top-level `clipboard_cmd` (string) and `osc52_limit` (integer) keys
/// from config.toml; missing/corrupt pieces degrade to defaults.
pub fn load_ui_settings(path: &std::path::Path) -> UiSettings;

// clipboard.rs
pub enum CopyResult {
    Copied { via: &'static str },     // "clipboard_cmd" | "clipboard" | "terminal (OSC 52)"
    OscTooLarge,                      // threshold hit on the OSC 52 tier — nothing sent
    Failed(String),
}
pub struct Clipboard { /* cmd, osc52_limit, lazily-initialized arboard handle,
                          test override to disable arboard */ }
impl Clipboard {
    pub fn new(settings: &crate::config::UiSettings) -> Self;
    #[cfg(test)] pub fn new_for_test(cmd: Option<String>, limit: usize, allow_arboard: bool) -> Self;
    pub fn copy(&mut self, text: &str) -> CopyResult;
}
```

Tier logic in `copy`:
1. `cmd` set → run `sh -c <cmd>`, write text to stdin, wait. Exit 0 → `Copied{via:"clipboard_cmd"}`; else `Failed(stderr-or-status)`. No size limit.
2. arboard: initialize once (cache the `Result`); `set_text` ok → `Copied{via:"clipboard"}`. Init/set failure falls through.
3. OSC 52: `text.len() >= osc52_limit` → `OscTooLarge` (send nothing). Else write `\x1b]52;c;{BASE64(text)}\x07` to stdout, flush → `Copied{via:"terminal (OSC 52)"}`; write error → `Failed`.

- [ ] **Step 1: Failing tests:**
  - `load_ui_settings`: file with `clipboard_cmd = "xclip"` and `osc52_limit = 1000` parses; missing file → defaults (65536, None); wrong types degrade
  - cmd tier: `new_for_test(Some("cat > $OUT".replace…), …)` using a temp file path baked into the command; copy "hello" → file contains "hello", result `Copied{via:"clipboard_cmd"}`
  - failing cmd (`"false"`) → `Failed`, does NOT fall through to other tiers (cmd is authoritative when set)
  - threshold: `new_for_test(None, 8, false)`; copy of 8+ bytes → `OscTooLarge`; 7 bytes → `Copied{via:"terminal (OSC 52)"}`
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.** Check `cargo tree -i crossterm` still shows one version after adding deps; prefer `arboard` with default features (if it drags in an incompatible transitive, disable default features and enable only what compiles — record the choice in the commit message).
- [ ] **Step 5: Commit** — `feat: tiered clipboard (clipboard_cmd, arboard, OSC 52 with size threshold)`

---

### Task 8: Response pane — view tabs and clickable JSON tree

**Files:**
- Modify: `crates/postui/src/components/response.rs`, `crates/postui/src/components/json_tree.rs` (only if a "row i is a container" accessor is missing), `crates/postui/src/action.rs`, `crates/postui/src/app.rs`

**Interfaces:**
- New actions: `Action::ResponseViewMode(crate::components::response::ViewMode)`, `Action::JsonRowClicked { row: usize, toggle: bool }`.
- Ready layout gains a tabs row under the summary: constraints become `[summary 1][tabs 1][hint 0/1][body Min][search 0/1]`. Tabs row renders, via `hit::chip`: `Tree` (`ResponseTab(ViewMode::Pretty)`, only when `view.tree.is_some()`), `Raw` (`ResponseTab(ViewMode::Raw)`), `Headers` (`ResponseTab(ViewMode::Headers)`); active tab accent+bold. The right side of this row is reserved for the copy buttons Task 9 adds — leave the space empty for now.
- `Response` needs public mutators for App: add `pub fn set_view_mode(&mut self, mode: ViewMode)` (delegates to `view.set_mode`, no-op when no view or oversize-blocked Pretty) and `pub fn click_row(&mut self, row: usize, toggle: bool)` (set `view.cursor = row` clamped; if `toggle` and Pretty mode: `tree.toggle(row)` + clamp + follow).
- Body rows in Pretty mode register per visible line: `Hit::JsonRow(i)` over the line, then `Hit::JsonArrow(i)` over the first two columns *only for container rows* (rows whose rendered line carries a `▸`/`▾` toggle — use the JsonTree API; if none exists, add `pub fn is_container_at_visible(&self, i: usize) -> bool`). In Raw/Headers modes register nothing per-row (Headers copy icons come in Task 9).
- `App::on_hit`: `ResponseTab(m)` → focus response + `update(ResponseViewMode(m))`; `JsonRow(i)` → focus response + `update(JsonRowClicked{row: i, toggle: false})`; `JsonArrow(i)` → same with `toggle: true`. `App::apply` implements both actions via the new mutators.
- Keyboard parity is already present (`r`/`h`/Enter) — no new bindings.

- [ ] **Step 1: Failing tests** (build a Ready response from a small JSON object as existing response tests do):
  - click `ResponseTab(ViewMode::Headers)` → headers view visible (`view().unwrap().mode == Headers`)
  - click `JsonArrow(i)` of a container row → visible_len shrinks (collapsed)
  - click `JsonRow(j)` of a scalar row → cursor == j, no collapse
  - oversize response (body > 2 MiB): Tree tab not registered (`rect_of(&Hit::ResponseTab(Pretty)) == None`)
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** (use the tmux http.server side-window trick to send a real request): tabs row reads cleanly, active tab obvious, arrows toggle on click, hover styling on tabs.
- [ ] **Step 6: Commit** — `feat: clickable response view tabs and JSON tree arrows`

---

### Task 9: Copy buttons, Save body to file

**Files:**
- Modify: `crates/postui/src/action.rs`, `crates/postui/src/app.rs`, `crates/postui/src/components/{editor.rs,response.rs,modal.rs,palette.rs}`, `crates/postui/src/main.rs` (App::new loads settings)

**Interfaces:**

```rust
// action.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget { ResponseBody, ResponseHeader(usize), Url }
Action::CopyToClipboard(CopyTarget)
Action::PromptSaveBody
Action::SaveBodyToFile(String)
// modal.rs
PromptKind::SaveBodyAs
```

- `App` gains `pub clipboard: crate::clipboard::Clipboard` and `pub ui_settings: UiSettings`; `App::new` loads settings from the same config.toml path as the registry; `bare()`/tests use defaults (`Clipboard::new(&UiSettings::default())`).
- `App::apply(CopyToClipboard(t))`: resolve text — `ResponseBody` → `ResponseState::Ready(d)`'s `d.body` (else toast "nothing to copy — send a request first", Warning); `ResponseHeader(i)` → `d.headers[i].1` (name for the toast); `Url` → `editor.url.text()`. Then match `clipboard.copy`:
  - `Copied{..}` → Success toast `Copied response body` / `Copied {header-name}` / `Copied URL`
  - `OscTooLarge` → Warning toast, exact string from Global Constraints
  - `Failed(_)` → Error toast, exact string from Global Constraints
- `App::apply(PromptSaveBody)`: no response → same "nothing to copy…" toast. Else push `Modal::Prompt { title: "Save response body", kind: SaveBodyAs, input: LineInput::new(&prefill) }` where prefill = `~/Downloads/{slug}-response.{ext}` (`slug` = `editor.slug` or `"response"`, with `/` mapped to `-`; ext = "json" if `content_type` contains "json" else "txt").
- `PromptKind::SaveBodyAs` Enter → `Action::SaveBodyToFile(text)`; apply: expand_tilde, `create_dir_all` parent, write body; Success toast `Saved body to {path}` / Error toast `could not save body: {e}`.
- Buttons:
  - Response tabs row, right-aligned: `[ Copy body ]` (`Hit::CopyBodyButton`) and `[ Save to file ]` (`Hit::SaveBodyButton`) — only in Ready state.
  - Headers view rows: register `Hit::HeaderCopy(i)` over a trailing ` ⧉` glyph appended to each header line (glyph accent-colored on hover).
  - Editor URL row: `[ Copy URL ]`-abbreviated as `[ ⧉ ]` (2-cell button) between URL and Send (`Hit::CopyUrlButton`) — keep the URL row uncluttered; the hover inversion plus footer/palette naming carries the meaning.
- `on_hit`: the four hits map to `CopyToClipboard(...)` / `PromptSaveBody`.
- Palette commands (keyboard parity, spec §9): `"Response: copy body"` → `CopyToClipboard(ResponseBody)`, `"Request: copy URL"` → `CopyToClipboard(Url)`, `"Response: save body to file…"` → `PromptSaveBody`. Header-row parity: in `ready_key`, `c` in Headers view → `Some(Action::CopyToClipboard(CopyTarget::ResponseHeader(view.cursor)))`.
- Test seam: give `App` `#[cfg(test)] pub fn set_clipboard_for_test(&mut self, c: Clipboard)`.

- [ ] **Step 1: Failing tests:**
  - copy body via a `new_for_test(Some(file-writing cmd), …)` clipboard → file holds the body; rendered toast contains `Copied response body`
  - OSC-too-large path (`new_for_test(None, 8, false)`, body ≥ 8 bytes) → rendered toast contains `Too large for the terminal clipboard`
  - `PromptSaveBody` prefill ends with `-response.json` for a json response; Enter writes the file to a tempdir path typed into the prompt; toast contains `Saved body to`
  - click `HeaderCopy(0)` → toast `Copied {first-header-name}`; `c` key parity in Headers view produces the same action
  - no response: `CopyToClipboard(ResponseBody)` → "nothing to copy" toast, clipboard untouched
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — buttons right-aligned on the tabs row without crowding; `⧉` on header rows; copy URL button; toasts read correctly.
- [ ] **Step 6: Commit** — `feat: copy buttons, tiered-clipboard actions, save body to file`

---

### Task 10: Draggable scrollbars

**Files:**
- Modify: `crates/postui/src/hit.rs` (scrollbar helper), `crates/postui/src/components/{sidebar.rs,response.rs,editor.rs}`, `crates/postui/src/app.rs`

**Interfaces:**

```rust
// hit.rs
pub struct ScrollbarSpec { pub pane: PaneId, pub offset: usize, pub content: usize, pub viewport: usize }
/// Renders a 1-cell-wide vertical scrollbar into `column` when content >
/// viewport: dim `│` track, accent `█` thumb (surface-on-accent when
/// hovered/dragged). Registers ScrollbarTrack(pane, -viewport as i16) above
/// the thumb, ScrollbarTrack(pane, +viewport as i16) below, and
/// ScrollbarThumb(pane) on the thumb (thumb last).
pub fn draw_scrollbar(frame, hits: &mut HitMap, column: Rect, spec: &ScrollbarSpec,
                      hovered: Option<&Hit>, dragging: bool, theme: &Theme);
/// (thumb_top_row_within_track, thumb_height) — proportional, min height 1.
pub fn thumb_geometry(spec: &ScrollbarSpec, track_h: u16) -> (u16, u16);
/// Inverse mapping used by drag: content offset for a desired thumb top.
pub fn offset_for_thumb_top(spec: &ScrollbarSpec, track_h: u16, thumb_top: u16) -> usize;
```

- Panes reserve their inner area's last column for the bar when content overflows: sidebar (`offset = self.scroll`, `content = rows.len()`, viewport = list height), response body area (`offset = view.scroll`, `content = visible_len()`, `viewport = view.height`). **Body pane:** first probe edtui (`~/.cargo/registry/src/*/edtui-*/src/`) for a public viewport/offset on `EditorState` (look for `view`, `viewport`, `offset`). If public: full spec. If not: render the bar with `offset = body.cursor.row`, `content = body.lines.len()`, viewport = area height (position indicator approximation), and implement drag via synthesized wheel deltas (below). Note which path you took in the commit message.
- `App` drag machinery (`Drag` struct exists from Task 2):
  - `on_hit(ScrollbarThumb(pane))` → store `Drag { pane, grab_offset: m.row - thumb_screen_top }`. Recompute thumb geometry from the same `ScrollbarSpec` the pane would draw — add `fn scrollbar_spec(&self, pane: PaneId) -> Option<ScrollbarSpec>` on App (sidebar/response/body as above) plus `fn scrollbar_track(&self, pane) -> Option<Rect>` derived from `hits.rect_of(&Hit::ScrollbarThumb(pane))`'s column… simpler: registers make the *track column rect* recoverable — also register the full column as `Hit::ScrollbarTrack` pieces; for geometry, store the track Rect on App during draw: `pub scrollbar_tracks: Vec<(PaneId, Rect)>` cleared each frame in `ui::draw` and pushed by `draw_scrollbar` via a small return value. Keep it simple: `draw_scrollbar` returns the track Rect; each pane's draw pushes `(pane, rect)` into a `&mut Vec` passed alongside hits — bundle both in a new `pub struct FrameArtifacts { pub hits: HitMap, pub tracks: Vec<(PaneId, Rect)> }` if threading two params is noisy. Choose one and keep it consistent.
  - `Moved` with `Some(drag)`: `let track = tracks entry; let new_top = m.row.saturating_sub(track.y + drag.grab_offset)` → `offset_for_thumb_top` → apply: sidebar → `sidebar.scroll = offset` (and `ensure_visible` stays false); response → `view.scroll = offset` clamped; body → issue wheel deltas: `delta_lines = target_offset as i32 - current_offset as i32`, forward via `editor.handle_scroll(delta)` in chunks. Return true when offset changed.
  - `Up` clears drag (Task 2 already does).
  - `on_hit(ScrollbarTrack(pane, delta))` → `update(Action::ScrollPane(pane, delta.clamp(-30, 30)))` (ScrollPane takes i16; viewport heights fit).
- Response `handle_scroll` already clamps; sidebar's free-scroll semantics are preserved (drag sets `scroll` directly, `ensure_visible = false`).

- [ ] **Step 1: Failing tests:**
  - `thumb_geometry`: content 100, viewport 10, track 10 → thumb height 1; offset 0 → top 0; offset 90 → bottom; round-trips with `offset_for_thumb_top`
  - sidebar with 30 rows in a short pane renders `█` and registers thumb + both track segments; a 5-row sidebar registers no scrollbar hits
  - drag simulation: render, Down on thumb rect, Moved +3 rows → `sidebar.scroll` increased proportionally; Up ends the drag (further Moved changes nothing)
  - track click below thumb → response `view.scroll` advanced by ~viewport
- [ ] **Steps 2–4: fail → probe edtui → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — bars visible only when needed, thumb proportional, drag with motion-event sequences actually scrolls, track click pages, body bar behaves.
- [ ] **Step 6: Commit** — `feat: draggable scrollbars in sidebar, response, body`

---

### Task 11: Mouse in modals — click rows, click-outside close, list scrolling

**Files:**
- Modify: `crates/postui/src/components/{modal.rs,chooser.rs,palette.rs,var_picker.rs}`, `crates/postui/src/app.rs`

**Interfaces:**
- `ModalStack::draw` registers `Hit::ModalOutside` over the whole screen before drawing the top modal (all variants — Dropdown already does; unify), then each modal's own hits.
- Row registration: `Modal::Palette` rows → `Hit::PaletteRow(i)` (i = index into `filtered`), `Chooser` → `Hit::ChooserRow(i)`, `VarPicker` → `Hit::VarPickerRow(i)`; `Confirm` renders its `[y] Label` hints as `hit::chip`s with `Hit::ConfirmChoice(char)`.
- **Remove the Task-2 guards**: `App::handle_mouse` no longer early-returns on Down when modals are open, and wheel with a modal open scrolls the modal's list (below) instead of no-op.
- `App::on_hit` modal arms (route through `modals.top_mut()`; after a `ModalResult`, pop on `close` and dispatch actions exactly as `handle_key` does — factor that 6-line block into `fn apply_modal_result(&mut self, res: ModalResult) -> bool` and reuse it in `handle_key`):
  - `ModalOutside` → `update(Action::Close)`
  - `PaletteRow(i)` → set selected = i and synthesize the Enter path (single click runs — spec §6)
  - `ChooserRow(i)` / `VarPickerRow(i)` → if `i` already selected **or** `clicks == 2` → Enter path; else select `i`
  - `ConfirmChoice(c)` → same as pressing `c`
- List scrolling (stage-3 carryover #1): `ChooserState`, `PaletteState`, `VarPickerState` each gain `scroll: usize`; draw windows the list (`skip(scroll).take(list_h)`), keeps the selection visible when it moves via keys (adjust scroll before windowing), resets scroll on refilter. Wheel over an open modal: `handle_mouse` Scroll arm, when modals non-empty, calls a new `ModalStack::scroll_top(&mut self, delta: i16)` that adjusts the top modal's list scroll (clamped; no-op for non-list modals). Selection can now never sit off-screen with >16-row lists. Row hits must be registered for *visible* rows with their true filtered index.
- Selection: hovered rows also get the `surface_raised` background like sidebar rows.

- [ ] **Step 1: Failing tests:**
  - open palette, click `PaletteRow` of the "Quit" row (filter first by pushing chars) → `should_quit`
  - open project chooser (two projects), click an unselected row once → selected moves, modal stays; click again → switch dispatched, modal closed
  - click outside the palette (a point outside its centered rect) → modal closed, no action
  - Confirm modal: click the `ConfirmChoice('y')` chip on delete-request confirm → file deleted
  - scrolling: chooser with 25 items in a 16-high modal — Down×20 keeps selection visible (`scroll` advanced); wheel via `handle_mouse` moves `scroll` without moving selection
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.** (The `ModalResult` struct gains no fields here; Task 12 adds `usage`.)
- [ ] **Step 5: tmux check** — click rows in palette/choosers, click-outside closes, confirm chips clickable, long lists scroll.
- [ ] **Step 6: Commit** — `feat: mouse support in modals; chooser/palette/var-picker list scrolling`

---

### Task 12: Palette frecency

**Files:**
- Create: `crates/postui/src/usage.rs` (+ `pub mod usage;`)
- Modify: `crates/postui/src/components/{palette.rs,modal.rs}`, `crates/postui/src/{config.rs,app.rs}`

**Interfaces:**

```rust
// usage.rs
#[derive(Default)]
pub struct UsageStore { /* HashMap<String, (count: u32, last_used: i64 unix secs)> */ }
impl UsageStore {
    pub fn load_from(path: &Path) -> Self;          // missing/corrupt → empty, never errors
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()>;
    /// Bump count + timestamp. When any count reaches 1000, halve ALL counts.
    pub fn record(&mut self, id: &str, now_secs: i64);
    /// count × 0.5^(age_days / 30); 0.0 for unknown ids.
    pub fn score(&self, id: &str, now_secs: i64) -> f64;
}
// config.rs
pub fn ui_file_path() -> Option<PathBuf>;           // <config dir>/ui.toml
```

TOML shape (`ui.toml`): `[palette.usage]` table, `"<id>" = { count = 12, last_used = 1786300000 }`.

- `Command` gains `pub id: &'static str` — assign kebab ids to every command (`focus-sidebar`, `focus-editor`, `focus-response`, `about`, `send`, `request-new`, `request-save`, `request-rename`, `request-delete`, `request-copy-url`, `method-cycle`, `method-choose`, `body-format`, `body-minify`, `body-external-editor`, `body-toggle-vars`, `project-choose`, `project-next`, `project-open-path`, `project-new`, `env-choose`, `env-next`, `vars-insert`, `response-copy-body`, `response-save-body`, `quit`).
- `PaletteState::new(usage: &UsageStore, now_secs: i64)` — sorts `all_commands()` by score descending (stable sort, so zero-score commands keep declaration order) and stores that as the base order; `refilter` filters the base order (fuzzy match unchanged) — empty query = frecency order, typed query = fuzzy-filtered in frecency order (frecency as tiebreak, spec §6). Fix all `PaletteState::new()` call sites (app.rs OpenPalette arm passes `&self.usage` and `now()`; tests pass `&UsageStore::default()`).
- `ModalResult` gains `pub usage: Option<String>` (default None — update the handful of struct literals). Palette Enter sets `usage: Some(id)`.
- `App` gains `pub usage: UsageStore`, private `usage_path: Option<PathBuf>` (None in tests, `config::ui_file_path()` in `App::new`); `apply_modal_result` records `res.usage` with `now()` (`std::time::SystemTime::now()` → unix secs helper); `Action::Quit` saves the store to `usage_path` (best-effort) alongside the existing persist.
- Mouse click on a palette row goes through the same Enter path (Task 11), so recording is automatic.

- [ ] **Step 1: Failing tests:**
  - `record`/`score`: recorded-now beats recorded-30-days-ago at equal counts; higher count wins at equal age; unknown id scores 0
  - halving: drive one id to 1000 → all counts halved
  - round-trip: record two ids, save, load → identical scores; corrupt file → empty store
  - ordering: store with "quit" heavily used → `PaletteState::new(&store, now).filtered()[0].id == "quit"`; typing "focus" still fuzzy-filters to the focus commands
  - execution records: run a palette command via Enter, assert `app.usage.score("quit", now) > 0.0`
- [ ] **Steps 2–4: fail → implement → green/clippy/fmt.**
- [ ] **Step 5: tmux check** — run a command twice via palette, reopen: it sits on top with an empty query.
- [ ] **Step 6: Commit** — `feat: palette frecency ranking persisted to ui.toml`

---

### Task 13: Stage-3 carryover fixes A (app behaviors)

**Files:**
- Modify: `crates/postui/src/{app.rs,config.rs,project_ctx.rs}`

Each fix lands with its regression test (write test → see it fail → fix → green):

- [ ] **1. `CreateProject` must not set `last` before the dirty gate resolves.** In the `CreateProject` arm, replace `self.registry.register(path)` with adding to `known` only (add `ProjectsRegistry::add_known(&mut self, path: PathBuf)` that pushes if absent, does NOT touch `last`), then save. `last` is set when `ForceSwitchProject` runs (it already calls `register`). Test: dirty editor → `CreateProject` → Esc the confirm → `registry.last` unchanged (still the old project), but the new path is in `known`.
- [ ] **2. `CycleEnv` performs the spec-§7 reload.** Add `self.apply(Action::ReloadProjectFiles);` as the first line of the `CycleEnv` arm (symmetry with `OpenEnvChooser`). Test: open project with env; rewrite `variables.toml` on disk + bump mtime (copy the `bump_mtime` helper pattern from project_ctx tests); `CycleEnv`; assert `app.project.variables` reflects the new file.
- [ ] **3. `ForceOpenRequest` persists `open_request`.** On the success path, after `editor.load`, `self.apply(Action::PersistLocalState);`. Test: `ForceOpenRequest("a")` → `load_local_state(root).open_request == Some("a")`.
- [ ] **4. Bare-root quit must not write `./.local/state.toml`.** Guard `ProjectContext::persist_local_state`: `if self.root.as_os_str().is_empty() { return; }`. Test the guard directly: build a `ProjectContext` via `ProjectContext::open(PathBuf::new())`, call `persist_local_state(None)` from a temp cwd-independent check — assert via a new tiny helper `ProjectContext::can_persist(&self) -> bool` (returns the guard condition) so the test doesn't depend on cwd: `assert!(!ctx.can_persist())`, and `persist_local_state` early-returns on `!can_persist()`.
- [ ] **5. `CycleProject` skips dead registry paths.** `ProjectsRegistry::next_after` walks forward (wrapping, at most `known.len()` steps) to the first entry that `is_dir()` and differs from `current`; returns None if none. Test: registry `[live_a, dead, live_b]`, current `live_a` → `live_b`.
- [ ] **6. Env-switch failure: single warning, no stale success toast.** In `SwitchEnv`, when `set_env` returns warnings (env unchanged), push the warnings and return — skip `PersistLocalState` and the `env: {label}` toast. Test: `SwitchEnv(Some("broken"))` with a corrupt env file → rendered text contains the warning but NOT `env:`.
- [ ] **Suite green, clippy, fmt. Commit** — `fix: stage-3 carryover — registry last, env reload symmetry, persistence guards`

---

### Task 14: Stage-3 carryover fixes B (slugify, CLI, prepare radar, stale edit)

**Files:**
- Modify: `crates/postui/src/components/modal.rs`, `crates/postui/src/app.rs`, `crates/postui/src/main.rs`, `crates/postui/src/config.rs`, `crates/postui-core/src/prepare.rs`

- [ ] **1. Empty-slug guards.** (a) NewProject Tab-prefill: if `slugify(name)` is empty, leave the path untouched (don't append). (b) `CreateProject` arm: `path.trim()` empty → Error toast `project path is empty — enter a path` and keep going nowhere (return true, modal already closed is fine). Tests: name `"日本語"` → path prefill unchanged after Tab; `CreateProject{name: "x", path: ""}` → toast, no project created, project root unchanged.
- [ ] **2. `postui --help` (any leading-dash arg).** Add to `config.rs`: `pub enum CliParse { Root(Option<PathBuf>), Usage }` and `pub fn parse_cli(arg: Option<String>) -> CliParse` — `Some(s)` starting with `-` → `Usage`; else existing expand_tilde behavior. `main()` calls it BEFORE `ratatui::init()`; on `Usage` print `usage: postui [directory]` and return Ok. Unit tests on `parse_cli` (`--help`, `-x`, normal path, none).
- [ ] **3. Prepare radar (read `crates/postui-core/src/prepare.rs` first; adapt names to what's there):**
  - (a) Default-header suppression must compare names **after** `{{var}}` substitution, case-insensitively — a template default-header name (`X-{{t}}`) must be suppressed by a request header whose name matches the substituted result. Test: ctx var `t=Api`, default header `X-{{t}}`, request header `x-api` present (enabled) → the default is not sent (overridden); disabled request header `x-api` → default suppressed entirely (existing disabled-row semantics).
  - (b) Case-differing duplicate default-header keys: after substitution, two defaults resolving to the same lowercase name → keep the first, emit a prepare warning `duplicate default header '<name>'` (reuse the existing warning channel `prepare` already returns). Test with `X-One` + `x-one`.
- [ ] **4. Stale `table.editing` must not capture `InsertVarText`.** Reorder the `InsertVarText` arm: the table-edit branch requires `self.focus == PaneId::Editor && matches!(self.editor.active_tab, EditorTab::Params | EditorTab::Headers) && self.editor.sub_focus == SubFocus::Content` in addition to `editing.is_some()`. Test: start a cell edit, `FocusPane(Response)`, `InsertVarText("x")` → "nowhere to insert" toast and the pending edit's input unchanged.
- [ ] **Suite green, clippy, fmt. Commit** — `fix: stage-3 carryover — slug guards, --help, prepare header radar, stale cell edit`

---

### Task 15: Stage-4 acceptance test

**Files:**
- Create: `crates/postui/tests/stage4_acceptance.rs` (follow the shape of the existing stage-3 acceptance test — find it under `crates/postui/tests/`)

One test driving a **mouse-only** flow end to end with `TestBackend` + `app.hits.rect_of` + synthesized `MouseEvent`s (re-render after every interaction so hits are fresh; helper `fn click(app, hit)` that renders, looks up the rect, sends Down at its center):

- [ ] **Step 1: Write the test (it should pass if Tasks 1–12 are correct — treat failures as bugs to fix, not test adjustments):**
  1. wiremock server with a JSON `POST /items` responder
  2. click `SidebarNewRequest` → prompt opens; type slug via `handle_key` (naming a request needs a keyboard — acceptable: text entry is not a mouse affordance) → request created
  3. click `MethodSelector` → click `DropdownRow` for POST → method badge POST
  4. click the URL area? (URL typing is keyboard) — type URL via keys
  5. click `EditorTab(1)` (Headers), click `EditorTab(0)` (Params); add a param via keys, click its `TableCheckbox(0)` off and on
  6. click `SendButton`; pump the tokio channel until `ResponseArrived` (mirror the stage-3 acceptance pumping pattern) → status 200 rendered
  7. click `ResponseTab(Headers)`, click `HeaderCopy(0)` with a file-backed test clipboard → file has the header value
  8. click `ResponseTab(Pretty)`, click a `JsonArrow` → node collapses
  9. click `Hit::FooterChip(Action::OpenPalette)` → click the `PaletteRow` for "Quit" → `should_quit`
- [ ] **Step 2: Run; fix any real defects it surfaces. Step 3: suite green, clippy, fmt.**
- [ ] **Step 4: Commit** — `test: stage-4 mouse-only acceptance flow`

---

### Task 16: Final polish — full tmux walkthrough, docs, sweep

**Files:**
- Modify: whatever the walkthrough surfaces; `crates/postui/src/app.rs` (About text), `README.md` (create if absent)

- [ ] **Step 1: Full mouse-only tmux walkthrough** of the acceptance flow (Global Constraints recipe) against a real `python3 -m http.server` side window: every surface from spec §2 — header selectors, sidebar (rows/arrows/new/scrollbar drag), tabs, method dropdown (including near-bottom flip), checkboxes, Send/Cancel, response tabs/tree/copy/save, footer chips, palette by mouse, click-outside, hover everywhere. Capture with `-e` and judge the styling yourself: alignment, spacing, hover inversion consistency, nothing clipped at 160×45 **and** at a small 80×24 session. Fix what's off.
- [ ] **Step 2: Document text selection (spec §8).** Add one line to the About modal body: `Text selection: hold Shift while dragging (mouse capture is on).` If `README.md` exists, add a short "Mouse" section (buttons, drag scrollbars, Shift+drag to select); if it doesn't exist, create a minimal one (name, one-paragraph description, build/run, the Mouse section, config keys `clipboard_cmd` / `osc52_limit`).
- [ ] **Step 3: Sweep** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`, `cargo tree -i crossterm` (one version).
- [ ] **Step 4: Commit** — `docs+polish: stage-4 walkthrough fixes, selection note, README`

---

### Task 17 (STRETCH — droppable): OSC 52 query-verify

Only attempt after everything else is merged-ready, and abandon without ceremony if it fights back (spec §7 marks it droppable): after a sub-threshold OSC 52 send, emit the read-back query `\x1b]52;c;?\x07`, watch the crossterm event stream ~150 ms for the response escape (crossterm may not surface it — probe first; if the events never arrive through `EventStream`, STOP and drop the task, noting why in the decisions log). On a response that decodes to different text → Warning toast `terminal clipboard may have truncated the copy`. No response → silence (can't verify ≠ failed).

---

## Self-review notes (already applied)

- Spec §2 modal behavior "click again or double-click confirms" → Task 11 implements click-selected-confirms OR double-click.
- Spec §5 keyboard path to the dropdown = "Choose method" palette command (Task 6) + existing cycle key untouched.
- Spec §9 parity: every new mouse affordance maps to an existing key, a palette command (copy/save/method), or a pane key (`c` for header copy).
- `Hit::FooterChip(Action)` embeds the action so no index bookkeeping can drift.
- Order matters: Task 7 (clipboard) before 9 (buttons); 8 (tabs row) before 9 (buttons live on that row); 2 before everything UI.
