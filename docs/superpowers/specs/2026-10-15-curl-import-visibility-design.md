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

## 2. Rules

| # | Behaviour | Source |
|---|---|---|
| V1 | A paste (ctrl+v, or a bracketed paste) while the sidebar has focus imports the pasted text's curl commands through the normal import (dirty gate, name prompt per command, summary toast). Both profiles. | you |
| V2 | That import, and the button's, goes into the cursor row's folder: a folder row is that folder; a request row is its request's folder; a request at the space root, or an empty sidebar, is the space root. | you |
| V3 | While the clipboard holds a curl command, the sidebar's top row splits into "+ New" and "Import curl" buttons. Otherwise it is the full-width "+ New request", as today. The list never moves. | you |
| V4 | The clipboard is checked at launch, when the terminal regains focus, and whenever postui reads or writes the clipboard itself (a paste, an import, Copy as curl or any other copy). It is never polled. | mine |
| V5 | "Holds a curl command" means `postui_core::curl::parse` accepts the text: the button shows only when clicking it would get as far as the name prompt. | mine |
| V6 | A click reads the clipboard fresh (it may have changed since the check). If the clipboard no longer parses, the click toasts the import's own error, as the palette command does, and the button goes away (that read is a V4 check). | mine |
| V7 | A sidebar paste that holds no curl command toasts the import's error ("clipboard doesn't hold a curl command", or the parse error). Today it is silent. | mine |
| V8 | When the halves are too narrow for "Import curl", its label shortens to "curl". When even that will not fit, the row stays "+ New request" with no import button. | mine |
| V9 | While the button shows, the sidebar's footer advertises the paste key: `^V import curl`. | mine |
| V10 | No button when the sidebar shows a notice (no project open), as with "+ New request". | mine |
| V11 | The button's hover hint is "Import curl from clipboard" (the palette row's wording). | mine |
| V12 | The palette, `:import` and the folder menu are unchanged (the palette and `:import` still import into the space root). | mine |

## 3. Design

**App** owns the check. A new `App::note_clipboard(text: Option<&str>)`
sets `self.sidebar.curl_ready` to `text.is_some_and(|t| parse(t).is_ok())`.
`App::check_clipboard()` reads the clipboard and calls it. It runs from:

- `main.rs` at startup and on `Event::FocusGained` (next to
  `ReloadProjectFiles`);
- `clipboard_curl` (every import or replace read);
- `Action::Paste`'s read;
- `copy_text_with_toast` with the copied text, so no read is needed.

**Sidebar** gets a display flag `curl_ready: bool`, written only by App,
as the jq bar's `ai_asking` is. `draw` splits the button row when it is
set (V3, V8) and registers a new `Hit::SidebarImportCurl` on the right
half.

**Paste routing.** `paste_text` gains one arm, after the modal and
response arms: `screen == Main && focus == Sidebar` →
`self.import_curl_text(text, self.sidebar_import_folder())`. `import_curl`
is split so that parsing works on given text:

- `curl_from_text(text) -> Option<CurlBatch>` holds today's parse-and-toast.
  `clipboard_curl` becomes read + `note_clipboard` + `curl_from_text`.
- `import_curl(folder)` = `clipboard_curl` then `start_curl_import(batch,
  folder)` (today's dirty gate and prompt). The paste arm calls
  `curl_from_text(text)` then `start_curl_import`.

`sidebar_import_folder()` maps the cursor row (V2). It returns `None`
for the space root, so the prompt prefill is unchanged.

**Click.** `Hit::SidebarImportCurl` →
`Action::ImportCurl { folder: self.sidebar_import_folder() }`.
`hint.rs` maps it to V11's caption.

**Footer.** The sidebar chip list takes the flag and appends
`(Key::Of(Action::Paste), "import curl", Some(Action::Paste))` when it is
set (V9).

Contracts: `Hit` gains one variant, and `Sidebar` gains one pub field.
No action, seam or core API changes. `CurlImportRun`, the prompt and the
create path are untouched.

## 4. Tests

- V1: sidebar focus + `set_read_for_test(curl)` + `Action::Paste` → an
  import prompt is up. Same through `paste_text` (the bracketed path).
  Same under the vim profile.
- V1 negative: the URL bar focused + a curl paste → the URL field gets
  the text, no prompt.
- V2: a folder row, a request in a folder, and a root request → each
  prompt's prefill (`users/…`, `users/…`, bare name).
- V3/V8/V10: sidebar render tests: flag off → "+ New request"; flag on →
  both labels and both hits; narrow → "curl"; very narrow → no import
  hit; a notice → neither.
- V4/V5: `note_clipboard` with curl, non-curl, broken curl (unclosed
  quote) and `None`. A copy sets it from the copied text. `check_clipboard`
  after a focus gain.
- V6: click with the clipboard swapped to non-curl → error toast, flag
  cleared, no prompt.
- V7: a non-curl sidebar paste → the error toast.
- V9: the footer with the flag on and off.
- Manual: tmux, the arrows profile: copy a curl, focus the terminal,
  see the split, click it, ctrl+v in the sidebar, then a folder cursor.

## 5. Mine, for you to veto

V4 (check points: launch, focus gain, postui's own clipboard traffic; no
polling), V5 (button only when the clipboard parses), V6 (fresh read on
click), V7 (non-curl sidebar paste toasts instead of staying silent),
V8 (narrow fallback), V9 (footer chip only while the button shows), V10,
V11, V12 (the palette and `:import` keep the space root).

## 6. Checklist

1. Spec committed.
2. `curl_from_text` / `start_curl_import` split; `note_clipboard` and
   `check_clipboard`; the tests for V4–V7.
3. The paste arm and `sidebar_import_folder`; the tests for V1 and V2.
4. The sidebar split, the hit, the hint and the click; the tests for V3,
   V8, V10 and V11.
5. The footer chip; the V9 test.
6. `main.rs` launch and focus-gain checks; the suite, clippy and fmt;
   the tmux check.
