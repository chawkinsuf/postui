# Text Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Users can select text with the mouse (drag) and keyboard (shift+arrows) in the body editor, the response view, and every single-line input, and copy the selection to the system clipboard with ctrl+c.

**Architecture:** Selection is managed by postui, modeless (the edtui body editor stays in Insert mode always; edtui's public `EditorState::selection` is used only as the paint + delete surface). `LineInput` grows a selection anchor that every single-line consumer inherits. The response view keeps a (visible-line, char-col) cell-range selection over whichever view mode is displayed. `App::handle_key` intercepts the ctrl+c Quit combo when any selection is active and copies via the existing 3-tier `Clipboard`. Mouse drags route through a new `App::text_drag` state parallel to the existing scrollbar `drag`.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, edtui 0.11.6 (`selection`, `DeleteSelection`, `EditorTheme::selection_style`), existing `clipboard.rs` (cmd → arboard → OSC 52).

**Spec:** This plan is the spec (user request: "select text, in the request body editor and the response view, as well as input text boxes etc").

## Global Constraints

- No modal editing surfaces to the user: the body editor's `mode` must read `Insert` after every postui-handled event (normalize after forwarding to edtui).
- ctrl+c stays Quit when no selection is active anywhere (keys.rs:159 reserved binding untouched).
- Selection semantics are GUI-like: click collapses, typing replaces, Backspace/Delete delete the selection, unshifted motion collapses to the appropriate edge.
- Body/response selections are cell-inclusive (the char under the drag head is included), matching edtui's `Selection::contains`. `LineInput` selections are half-open char ranges `[start, end)`.
- Copy leaves the selection in place (GUI convention); it does not clear it.
- Paste is explicitly out of scope (no bracketed paste yet — follow-up).
- All new tests: `cargo test` must stay green after every task.

---

### Task 1: `LineInput` selection core

**Files:**
- Modify: `crates/postui/src/components/line_input.rs`

**Interfaces:**
- Produces: `LineInput::selection() -> Option<(usize, usize)>` (half-open char range, start < end), `selected_text() -> Option<String>`, `select_all()`, `clear_selection()`, `begin_mouse_selection()` (anchor = cursor), `set_cursor_extending(idx)` (moves cursor keeping anchor). `set_cursor` and `insert_str` clear the anchor.

**Steps:**

- [ ] **Step 1: Failing tests** — in line_input.rs tests: shift+Right from 0 on "abc" selects `(0,1)` cursor 1; shift+Home from end selects all reversed; unshifted Left with selection collapses to start without moving past it; typing `x` with `(0,2)` selected on "abc" yields "xc" cursor 1; Backspace with selection deletes only the selection; Delete likewise; ctrl+a selects `(0, len)`; `selected_text()` returns the slice; empty anchor==cursor is `None`.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement** — add `anchor: Option<usize>` field. In `handle_key`: SHIFT+Left/Right/Home/End set anchor (if none) then move cursor; unshifted motion with a live selection collapses (Left/Home → start, Right/End → end... Home/End still go to 0/len) and clears anchor; Char/insert first deletes a live selection; Backspace/Delete with a live selection delete it and return; ctrl+a (CONTROL modifier, `KeyCode::Char('a')`) selects all. `selection()` normalizes anchor/cursor order, `None` when equal.
- [ ] **Step 4: Selection painting** — in `draw_line`/`draw_line_masked`/`render_windowed` (all caret-drawing paths): cells in the selected range render with `Modifier::REVERSED` (same mechanism as the caret; the caret cell keeps its look). Windowed drawing crops the range to the visible window. Test: TestBackend paint asserts REVERSED on selected cells and not on unselected ones.
- [ ] **Step 5: Full run + commit** — `feat: LineInput selection (shift+arrows, ctrl+a, replace/delete semantics)`.

---

### Task 2: ctrl+c copies the active selection (App plumbing)

**Files:**
- Modify: `crates/postui/src/app.rs` (`handle_key` step 1 at app.rs:3840, new helpers), `crates/postui/src/components/modal.rs` (focused-input accessor), `crates/postui/src/components/table_editor.rs` (editing-input accessor if not public), tests in `crates/postui/src/app/tests.rs`.

**Interfaces:**
- Produces: `App::active_selection_text(&self) -> Option<String>` — priority: top modal's focused `LineInput` → varmanager form/grid editing input → (Main screen) table cell edit → URL input → body editor (Task 3) → response (Task 5). `App::copy_text_with_toast(&mut self, text: String)` extracted from the `Action::CopyToClipboard` arm (app.rs:742-760) so both paths share toasts.
- Consumes: `LineInput::selected_text()` from Task 1.

**Steps:**

- [ ] **Step 1: Failing test** — app test: focus URL (sub_focus Url), give the URL input a selection via shift+Right keys, send ctrl+c through `App::handle_key`; assert the app did **not** quit (no quit flag/modal) and the test clipboard captured the selected text; selection still present. Second test: ctrl+c with no selection anywhere still quits (existing behavior — assert whatever `Action::Quit` does today, e.g. quit gate/flag).
- [ ] **Step 2: Run, verify failure.**
- [ ] **Step 3: Implement** — in `handle_key` step 1: `if modified && global == Some(Action::Quit) { if let Some(text) = self.active_selection_text() { self.copy_text_with_toast(text); return true; } return self.update(Action::Quit); }`. Add `ModalStack::focused_input(&self) -> Option<&LineInput>` (Prompt / NewProject / MultiPrompt focused field). Wire the priority chain (only the inputs that exist so far; body/response arms come in later tasks).
- [ ] **Step 4: Run all tests + commit** — `feat: ctrl+c copies the active selection, falls back to quit`.

---

### Task 3: Body editor mouse-drag selection

**Files:**
- Modify: `crates/postui/src/components/editor.rs`, `crates/postui/src/app/mouse.rs`, `crates/postui/src/app.rs` (`text_drag` field), `crates/postui/src/theme/mod.rs` (selection color token).

**Interfaces:**
- Produces: `App::text_drag: Option<TextDrag>` where `enum TextDrag { Body, Url, Response }` (mouse.rs, next to `Drag`); `Editor::body_sel_anchor: Option<edtui::Index2>`; `Editor::body_drag_to(col, row) -> bool` (maps via `body_cursor_for_click`, sets cursor + `body.selection = Selection::new(anchor, cursor)`, `None` when equal); `Editor::body_selected_text() -> Option<String>`; `Editor::clear_body_selection()`; `Theme::selection` color (blend of accent toward panel, distinct from `control_hover`).
- Consumes: edtui `Selection` (inclusive end), `EditorTheme::selection_style`.

**Steps:**

- [ ] **Step 1: Failing tests** (editor tests):
  - Down(Left) in the body area then `body_drag_to` two cells right → `body.selection == Some(anchor..cursor)`, `body_selected_text()` returns the covered chars (inclusive of the head cell).
  - A plain Down(Left) afterwards clears the selection.
  - After the drag, `body.mode == EditorMode::Insert` (modeless invariant).
  - Multi-line drag joins with `\n`.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement editor side** — on Down(Left) in `handle_mouse`: clear selection, then after the existing caret override set `body_sel_anchor = Some(cursor)`. Add `body_drag_to` (clamp via the same `body_cursor_for_click`; ignore when the point maps outside). Normalize mode to Insert at the end of `handle_mouse` and after `body_handler` forwarding. `body_selected_text` walks `body.lines` rows from `selection.start()` to `selection.end()` inclusive.
- [ ] **Step 4: Route drags in App** — mouse.rs Down(Left) `on_hit` `Hit::BodyEditor` arm (or post-`on_hit`): set `self.text_drag = Some(TextDrag::Body)`. In the `Moved | Drag(Left)` branch (mouse.rs:37-50): after the scrollbar-drag check, `if kind == Drag(Left) && let Some(td) = self.text_drag` route: Body → `self.editor.body_drag_to(col, row)`. `Up(Left)` clears `text_drag` too. App-level test: synthesize Down + Drag `MouseEvent`s through `App::handle_mouse`, assert selection exists.
- [ ] **Step 5: Paint** — add `Theme::selection` (both dark/light ctors; blend accent→panel ~35% via the existing blend helper near theme/mod.rs:455). In the `EditorView` theme construction (editor.rs:1648): `.selection_style(Style::default().bg(theme.selection).fg(theme.text))`. Paint test: draw with a selection set, assert a selected cell's bg is `theme.selection`.
- [ ] **Step 6: Wire copy** — add body arm to `App::active_selection_text` (only when `editor.sub_focus == Content`... include also when editor pane focused). Test: drag-select then ctrl+c copies the body text.
- [ ] **Step 7: Full run + commit** — `feat: mouse-drag text selection in the body editor`.

---

### Task 4: Body editor keyboard selection

**Files:**
- Modify: `crates/postui/src/components/editor.rs` (SubFocus::Content key arm, editor.rs:790-810), tests there + app/tests.rs.

**Interfaces:**
- Consumes: `body_sel_anchor`, `clear_body_selection`, edtui `DeleteSelection` (`edtui::actions::DeleteSelection` via `Execute`).
- Produces: shift+Arrow/Home/End extend selection; typing replaces; Backspace/Delete delete; Esc clears selection before it blurs; ctrl+a selects the whole body.

**Steps:**

- [ ] **Step 1: Failing tests:**
  - shift+Right from (0,0) over "ab" → selection covers 'a', cursor (0,1).
  - shift+Down extends across lines using the same wrap-aware motion as unshifted (reuse: strip SHIFT, run existing nav path, then set selection).
  - Unshifted Right with a selection clears it and moves normally.
  - Typing 'x' with a selection: selection text replaced by 'x', mode Insert.
  - Backspace with a selection deletes only it.
  - Esc with a selection clears it and keeps `sub_focus == Content`; next Esc blurs (existing behavior).
  - ctrl+a selects from (0,0) to the last char; `body_selected_text()` == full body.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement** in the Content arm, before `body_nav_key`:
  1. `ctrl+a` → anchor (0,0), selection to `(last_row, last_col)`, return.
  2. SHIFT + (arrows/Home/End): ensure anchor (= cursor if none); rebuild the event without SHIFT; run it through the existing `body_nav_key` + `body_handler` flow; then set `body.selection = Selection::new(anchor, cursor)` (None if equal). Return.
  3. Selection active + Esc → clear selection, return (blur only when no selection).
  4. Selection active + Backspace/Delete → `DeleteSelection.execute(&mut self.body)`, clear anchor, force mode Insert, return.
  5. Selection active + plain Char/Enter → `DeleteSelection` first, force Insert, then fall through so the char inserts at the collapsed cursor.
  6. Selection active + any other motion/edit → clear selection, fall through.
- [ ] **Step 4: Run all tests + commit** — `feat: keyboard selection in the body editor (shift+motions, ctrl+a, GUI replace semantics)`.

---

### Task 5: Response view selection (all three view modes)

**Files:**
- Modify: `crates/postui/src/components/response.rs`, `crates/postui/src/app/mouse.rs`, `crates/postui/src/app.rs`.

**Interfaces:**
- Produces: on `ReadyView`: `sel: Option<(Cell, Cell)>` with `type Cell = (usize /*visible line*/, usize /*char col*/)` (anchor, head — head cell inclusive); `Response::begin_selection_at(col, row) -> bool`, `Response::drag_selection_to(col, row) -> bool` (both map screen → content coords using the recorded content area + `scroll`/`h_scroll`); `Response::selected_text() -> Option<String>`; `Response::clear_selection()`; `Response::select_line_extend(delta: isize)` for shift+Up/Down; `ReadyView::display_line_text(mode, i) -> Option<String>` (Raw → `raw_lines[i]`, Headers → `header_lines[i]`, Pretty → `tree.visible_lines()[i]` text as painted — verify the painted row prefix (expand arrow/indent) matches and compensate any fixed prefix offset so screen col ↔ char col agree).
- Consumes: the search-match span-splitting mechanism (`highlighted` / `match_ranges` pattern at response.rs:337, 1060) for painting; `Theme::selection` from Task 3.

**Steps:**

- [ ] **Step 1: Failing tests:**
  - `begin_selection_at` + `drag_selection_to` on a Raw view maps through `scroll`/`h_scroll` to the right cells; `selected_text()` returns the inclusive-cell slice; multi-line joins with `\n`.
  - Drag col past line end clamps to line end; drag onto an empty line yields the line break only.
  - Painting: selected cells get `bg == theme.selection` in `body_lines`, cropped correctly when `h_scroll > 0` (only visible part painted, no panic).
  - Switching view mode (`set_mode`/tab switch) and loading a new response clear `sel`.
  - Click (Down without drag) clears any selection.
  - shift+Down with response focused extends a line-wise selection (cursor line → col 0..line end of the new cursor line).
  - Pretty mode: selecting across a row returns the on-screen text (accounting for the row's painted prefix).
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement state + mapping** — record the content `Rect` at draw time (alongside the existing `height`/`width` bookkeeping at response.rs:812/826). Screen→cell: `line = scroll + (row - area.y)`, col from display-width walk of the line text starting at `h_scroll` (use `unicode_width` like `crop_cols`); clamp line to last displayed line, col to line char count.
- [ ] **Step 4: Implement painting** — in `body_lines`'s per-line closure, compute this line's selected char range from the normalized `(anchor, head)` (full-width for interior lines, partial at the ends, head cell inclusive → end+1) and split spans with `bg: theme.selection` (reuse/generalize the search-highlight splitter). Order: selection styling over search-match styling.
- [ ] **Step 5: Route mouse** — `on_hit` arms for `Hit::Pane(PaneId::Response)`, `Hit::JsonRow(_)` (Down only; JsonArrow keeps toggling): call `response.begin_selection_at(col,row)`, set `text_drag = Some(TextDrag::Response)`, keep existing focus/cursor behavior. Drag branch routes `TextDrag::Response` → `drag_selection_to`. Add these hits to the `keeps_table_selection`-adjacent allowlists only if a regression test shows commits firing (they already pass through today).
- [ ] **Step 6: Keyboard + copy** — response `handle_key`: shift+Up/Down → `select_line_extend(±1)`; Esc clears selection first if present. Add the response arm (focused Response pane) to `App::active_selection_text` via `response.selected_text()`.
- [ ] **Step 7: Full run + commit** — `feat: text selection in the response view (mouse drag, shift+up/down, all view modes)`.

---

### Task 6: URL bar mouse-drag selection

**Files:**
- Modify: `crates/postui/src/app/mouse.rs` (`Hit::UrlBar` arm at mouse.rs:634, drag routing), `crates/postui/src/components/editor.rs` (expose the url text area mapping), app tests.

**Interfaces:**
- Consumes: `LineInput::begin_mouse_selection()`, `set_cursor_extending(idx)`, `select_all()` from Task 1; `Editor::last_url_text_area`.
- Produces: single click places the caret (existing) and sets the mouse anchor; drag extends the selection; double-click selects the whole URL.

**Steps:**

- [ ] **Step 1: Failing tests** — app-level: Down on the URL bar then Drag two cells right selects those chars (assert `url.selection()`); double-click (clicks == 2) selects the entire URL; a later plain click collapses.
- [ ] **Step 2: Run, verify failures.**
- [ ] **Step 3: Implement** — in the UrlBar arm: after the existing `set_cursor(start + col)`, call `begin_mouse_selection()`; on `clicks == 2` call `select_all()` instead; set `text_drag = Some(TextDrag::Url)`. Drag routing: recompute the char index from the drag x (same `window_start` math, clamped to the text area) and call `set_cursor_extending(idx)`. Selection painting comes free from Task 1's windowed draw.
- [ ] **Step 4: Full run + commit** — `feat: mouse selection in the URL bar (drag + double-click select-all)`.

---

### Task 7: Selection wiring for table cells, modals, var manager

**Files:**
- Modify: `crates/postui/src/app.rs` (`active_selection_text` arms), `crates/postui/src/components/modal.rs`, `crates/postui/src/components/varmanager.rs` (accessors if missing), tests.

**Interfaces:**
- Consumes: Task 1 (`LineInput` selection works in these inputs already via their existing key forwarding — table `handle_editing_key` at table_editor.rs:442, modal prompt fields, varmanager form/grid).

**Steps:**

- [ ] **Step 1: Failing tests** — (a) editing a table cell, shift+arrows select, ctrl+c copies the cell selection instead of quitting; (b) same for a modal Prompt field (ctrl+c intercepts *before* modal capture — verify explicitly since `handle_key` step 1 runs before step 2); (c) var-manager form field on the VarManager screen (before the screen-capture branch).
- [ ] **Step 2: Run, verify failures** (some may already pass from Task 2's chain — verify which and keep only meaningful asserts).
- [ ] **Step 3: Implement** any missing accessors and `active_selection_text` arms; confirm selection paints in each surface (they all draw via Task 1's paths — spot-check TestBackend paint for a modal field).
- [ ] **Step 4: Full run + commit** — `feat: selection copy wiring for table cells, modal fields, var manager`.

---

### Task 8: Verification sweep

**Steps:**

- [ ] **Step 1:** `cargo test` full workspace, `cargo clippy --all-targets` clean (match repo's existing lint bar).
- [ ] **Step 2:** tmux real-terminal smoke (memory: tmux-tui-driving recipe): drag-select in body → highlighted; drag in response Raw + Pretty; shift+arrows in URL; ctrl+c shows "copied" toast (OSC 52 may be refused inside tmux — toast/no-crash is the assertion); ctrl+c with nothing selected still quits (gated on unsaved changes as before).
- [ ] **Step 3:** Update the plan checkboxes; final commit if fixes were needed.
