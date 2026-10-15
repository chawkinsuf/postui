# Curl import visibility (task #9)

## 1. Problem

Curl import (spec `2026-10-11-curl-import-export-design.md`) is reachable
only from the palette ("Request: import curl from clipboard"), the `:import`
ex command and a folder's right-click "Import curl here…". Nothing on
screen says it exists, and the gesture people try first, pasting, does
nothing:

- `Action::Paste` (ctrl+v) and a terminal bracketed paste
  (`Event::Paste`: cmd+V on macOS, ctrl+shift+V on Linux) both go to
  `App::paste_text`, which returns `false` when no text field has the
  caret. With the sidebar focused, the paste is dropped without a word.
- The sidebar's only control is the full-width "+ New request" button
  (`Sidebar::draw`, `Hit::SidebarNewRequest`).

That spec's R5 ("No new keys. Palette commands and right-click rows
only.") was mine. This task replaces it (you, 2026-10-15).

### Amendment after review block 1 (2026-10-15)

Rounds 1–3 kept finding faults in the background clipboard read behind
the button: its cost on every focus gain and copy, a copy's read-back
racing an asynchronous `clipboard_cmd`, and above all macOS 26's
pasteboard privacy, which alerts the user when an app reads the
clipboard in code rather than on a paste (Apple's AppKit notes; only
the `NSPasteboard` "detect" APIs, which arboard does not use, are quiet).
A read on every focus gain could alert on every switch back to the
terminal. You chose (2026-10-15): the detected button becomes an opt-in
setting, off by default, and the read after a copy goes. The paste
(V1, V2, V7) is unchanged.

## 2. Rules

| # | Behaviour | Source |
|---|---|---|
| V1 | A paste (ctrl+v, or a bracketed paste) while the sidebar has focus imports the pasted text's curl commands through the normal import (dirty gate, name prompt per command, summary toast). Both profiles. | you |
| V2 | That import, and the button's, goes into the cursor row's folder: a folder row is that folder; a request row is its request's folder; a request at the space root, or an empty sidebar, is the space root. | you |
| V3 | With the V13 setting on, while the clipboard holds a curl command, the sidebar's top row splits into "+ New" and "Import curl" buttons. Otherwise it is the full-width "+ New request", as today. The list never moves. | you |
| V4 | With the setting on, the clipboard is checked at launch, when the terminal regains focus, when the setting is turned on, and on every import's or replace's own read. Not on a paste (it would parse every paste into a text field for nothing, round 1) and not after a copy (a read-back races an asynchronous `clipboard_cmd`, round 3). Never polled. With the setting off, postui never reads the clipboard unasked. | mine, the setting yours |
| V5 | "Holds a curl command" means `postui_core::curl::parse` accepts the text: the button shows only when clicking it would get as far as the name prompt. | mine |
| V6 | A click reads the clipboard fresh (it may have changed since the check). If the clipboard no longer parses, the click toasts the import's own error, as the palette command does, and the button goes away. A sidebar ctrl+v is that same click (round 2). | mine |
| V7 | A sidebar paste that holds no curl command toasts the import's error ("clipboard doesn't hold a curl command", or the parse error). Today it is silent. | mine |
| V8 | Each button gets its label's width plus half the spare columns, a column apart. When "Import curl" will not fit, its label shortens to "curl". When even that will not fit, the row stays "+ New request" with no import button. | mine |
| V9 | The sidebar's footer always lists `^V import curl`, last. With the setting off by default, the chip is what shows the paste; it no longer depends on the clipboard, so it needs no check. | mine |
| V10 | No button when the sidebar shows a notice (no project open), as with "+ New request". | mine |
| V11 | The button's hover hint is "Import curl from clipboard" (the palette row's wording). | mine |
| V12 | The palette, `:import` and the folder menu are unchanged (the palette and `:import` still import into the space root). | mine |
| V13 | A `curl_button` setting (`config.toml`, a checkbox row "Import curl button" on the Settings tab, before "Clipboard command"), default off, turns V3–V4 on. Turning it off hides the button at once. | you (opt-in, default off); the name and place mine |
| V14 | A clipboard that can't be read toasts "clipboard read failed: …" (the cause), as ctrl+v into a text field does, not "clipboard doesn't hold a curl command". This covers import and replace from every entry point. | mine (rounds 1 and 3) |

## 3. Design

**App** owns the check. `read_clipboard_curl() -> Result<CurlBatch,
String>` is the one clipboard read and parse: a read error is
`clipboard read failed: {e}` (V14), a parse error its message. It
records the answer in `self.sidebar.curl_ready`, but only while
`curl_button` is on, so the import's own reads keep the button honest
without ever showing it while the setting is off. `check_clipboard() ->
bool` runs it when the setting is on (and clears the flag when it is
off) and says whether the answer changed (a redraw). It runs from:

- `main.rs` at startup, and `App::on_focus_gained` (with
  `ReloadProjectFiles`, which `main.rs` ran there before) on
  `Event::FocusGained`;
- the `curl_button` toggle, wherever config changes land (the Settings
  row, undo of it, a config reload).

Cost: a 5 MB clipboard that is not curl parses in 42 ms in a release
build (measured in review round 1); a clipboard of a few KB is well
under a millisecond.

**Sidebar** gets a display flag `curl_ready: bool`, written only by App,
as the jq bar's `ai_asking` is. `offers_import()` is the flag with no
notice (V10). `draw` splits the button row when it is set (V3, V8) and
registers a new `Hit::SidebarImportCurl` on the right half.
`Sidebar::import_folder(space)` maps the cursor row (V2); it returns
`None` for the space root, so the prompt prefill is unchanged.

**Paste routing.** `sidebar_has_paste()` (no modal, Main, sidebar
focused) decides both paths; no vim field is live there (`screen_field`
is `None` on the sidebar), so the vim seam never claims the paste
first. A ctrl+v there is the button's click, `import_curl_at_cursor()`.
A terminal's bracketed paste carries its own text, so `paste_text` gains
one arm, after the modal and response arms: `import_pasted_curl(text)`.
Both end in one `import_parsed(parsed, folder)`: the project gate, the
error toast, then `start_curl_import` (today's dirty gate and prompt).
`import_curl(folder)` (the palette, `:import`, the folder menu) is
`read_clipboard_curl` then `import_parsed`.

A test that pinned "a sidebar paste is unhandled"
(`bracketed_paste_routes_to_the_focused_input`) now checks the method
badge, which still takes no text.

**Click.** `Hit::SidebarImportCurl` → `import_curl_at_cursor()`.
`hint.rs` maps it to V11's caption. (The palette row's description, the
way "+ New request" gets its hint, is over the hints' 42-column limit.)

**Footer.** The sidebar's static chip list ends with
`(Key::Of(Action::Paste), "import curl", Some(Action::Paste))` (V9). The
footer functions keep their signatures. Two hover-hint layout tests
sized their terminal to the sidebar's chip row; their 134 columns
become 153, the same gap with the new chip in it.

**Setting.** `UiSettings::curl_button` follows `hover_hints` through
`config.rs` (parse, template, warnings), `SettingsField::CurlButton`
(key, label, checkbox, row order), `hint.rs` and the App's config write,
undo and reload arms.

Contracts: `Hit` gains one variant, `Sidebar` one pub field plus
`offers_import` and `import_folder`, `UiSettings` one field and
`SettingsField` one variant. No action, seam or core API changes.
`CurlImportRun`, the prompt and the create path are untouched.

## 4. Tests

- V1: sidebar focus + `set_read_for_test(curl)` + `Action::Paste` → an
  import prompt is up. Same through `paste_text` (the bracketed path).
  Same under the vim profile.
- V1 negative: the URL bar focused + a curl paste → the URL field gets
  the text, no prompt.
- V2: a folder row, a request in a folder, and a root request → each
  prompt's prefill.
- V3/V8/V10: sidebar render tests: flag off → "+ New request"; flag on →
  both labels and both hits; narrow → "curl"; very narrow → no import
  hit; a notice → neither.
- V4/V5/V13: with the setting on, `check_clipboard` with curl, non-curl,
  broken curl and an unreadable clipboard; `on_focus_gained` rechecks.
  With it off, no check and no button, even after an import read curl.
  Turning it on checks; turning it off hides. A copy does not check.
- V6: a click, and a ctrl+v, with the clipboard swapped to non-curl →
  error toast, flag cleared, no prompt.
- V7: a non-curl sidebar paste → the error toast.
- V9: the sidebar footer lists the chip whatever the clipboard holds.
- V14: an unreadable clipboard → "clipboard read failed: …" from the
  palette import and from a sidebar ctrl+v.
- Manual: tmux, the arrows profile: the paste at the root and in a
  folder; the Settings row; the button after a copy is not expected.

## 5. Mine, for you to veto

V4's check points, V5, V6, V7, V8, V9 (the chip always shows now), V10,
V11, V12, V13's name (`curl_button`) and place (before "Clipboard
command"), V14.

## 6. Checklist

1. Spec committed (d992d43); amended after review block 1.
2. `read_clipboard_curl`, `import_parsed`, `check_clipboard`; the tests
   for V4–V7 and V14.
3. The paste routing and `Sidebar::import_folder`; the tests for V1, V2.
4. The sidebar split, the hit, the hint and the click; the tests for V3,
   V8, V10 and V11.
5. The footer chip; the V9 test.
6. The `curl_button` setting; the V13 tests.
7. `main.rs` launch and `on_focus_gained`; the suite, clippy and fmt;
   the tmux check.
