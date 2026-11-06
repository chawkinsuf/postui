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

### Amendment for #240 (2026-11-05)

With `+ Folder` in the sidebar's top row (create folders F10), the curl
button made the row tight. In a chat design session the button became a
plain toast (V3), and the rows that named the button are amended in
place. Removed: `Hit::SidebarImportCurl` and its click and hint,
`Sidebar::curl_ready` and `offers_import`, and the three-button layouts.
Section 3 below describes the button as built for #9. Two of my choices
from the #240 design round no longer apply, because the notice has no
button: which folder the card's Import button imports into, and where
the card sits in the toast stack. The notice is an ordinary toast in the
stack, and ctrl+v on the sidebar imports into the cursor row's folder
(V2).

*Amended again 2026-11-05 (#240, after review round 1):* a plain toast
gave a mouse user nothing to click. You: "maybe a new type of toast
panel is warrented". The notice is now a toast type of its own in the
same stack, with an Import button and a × (V3), and the rows below say
so. Ordinary toasts are unchanged. Added: `Hit::CurlNotice` with
`NoticePart` (panel, Import, ×), and `Toasts::push_curl_notice`.

## 2. Rules

| # | Behaviour | Source |
|---|---|---|
| V1 | A paste (ctrl+v, or a bracketed paste) while the sidebar has focus imports the pasted text's curl commands through the normal import (dirty gate, name prompt per command, summary toast). Both profiles. | you |
| V2 | That import, and the button's, goes into the cursor row's folder: a folder row is that folder; a request row is its request's folder; a request at the space root, or an empty sidebar, is the space root. *Amended 2026-11-05 (#240):* there is no button any more; the sidebar paste keeps this rule. *Amended again 2026-11-05 (#240, review round 1):* the V3 notice's Import button follows it too. | you |
| V3 | With the V13 setting on, while the clipboard holds a curl command, the sidebar's top row splits into "+ New" and "Import curl" buttons. Otherwise it is the full-width "+ New request", as today. The list never moves. *Amended 2026-11-05 (#240):* the button is gone and the sidebar row is `+ Request \| + Folder` for good (create folders F11). In its place, a check (V4) that finds curl the import would take (V5), and that the last notice did not show, shows a plain informational toast in the toast corner: "Clipboard holds a curl command — ^V in the sidebar imports it", the key spelled for the profile as the sidebar's footer spells it. It has no button, no × and no click target, and fades on the toast timer like any other toast. It never shows again for the same text, even after other text held the clipboard in between; a different curl shows it again. The memory is a hash of the last text, trimmed and taken line by line without the line ends, in `App::curl_noticed`, so other line endings or a trailing newline do not make it new. *Amended again 2026-11-05 (#240, review round 1):* the plain form above gave a mouse user nothing to click, so the notice is now a toast type of its own in the same stack, replacing "no button, no × and no click target". It reads "Clipboard holds a curl command", then an `Import` button, the hint "or ^V in the sidebar" (the key spelled for the profile; left out when the key is unbound or the notice is too narrow for it), and a `×`. Import does what a sidebar ctrl+v does (V2, V6, and V10 for where it is refused); × takes the notice down. It lives about 10 s. While the pointer is on it its clock stops, a fade already begun undone, and once the pointer leaves it runs on with the time it had left, then fades. A newer notice takes the place of one still showing, since it speaks of the clipboard as it is now. It never comes back for the same text (the memory above). Ordinary toasts are unchanged. *Amended 2026-11-05 (#240, second review block, round 1):* Import imports the text the notice reported, which the notice keeps, not what the clipboard holds at the click (V6). The pointer holds the notice only once it has moved onto it: a pointer that came to rest where the notice was then drawn lights it but does not hold it, and one that leaves must move back onto it to hold it again. Under a dialog or the edit flag's waiting screen the notice waits (V10). | the toast corner mine (approved: "just in the toast like you originally recommended"); the plain form mine (approved); fading and no repeat you ("have it disappear and not come back unless theres a new one"); the clickable toast type you ("maybe a new type of toast panel is warrented"); its contents, the 10 s and the pause under the pointer mine (approved); one notice at a time mine; Import taking the reported text and the hold on motion only mine |
| V4 | With the setting on, the clipboard is checked at launch, when the terminal regains focus, when the setting is turned on, and on every import's or replace's own read. Not on a paste (it would parse every paste into a text field for nothing, round 1) and not after a copy (a read-back races an asynchronous `clipboard_cmd`, round 3). Never polled. With the setting off, postui never reads the clipboard unasked. *Amended 2026-11-05 (#240):* the check points stay. A check at launch, on a focus gain or on turning the setting on may show the V3 notice. An import's or replace's own read never shows it, but curl it reads is marked as known, so a later check does not announce curl the user just imported. Every check reads, wherever it runs (V10), so turning the setting on from the Settings tab shows the notice there. | mine, the setting yours; the #240 changes mine |
| V5 | "Holds a curl command" means `postui_core::curl::parse` accepts the text: the button shows only when clicking it would get as far as the name prompt. *Amended 2026-11-05 (#240):* the same test decides the V3 notice: curl past the import's 50-command limit does not count. | mine |
| V6 | A click reads the clipboard fresh (it may have changed since the check). If the clipboard no longer parses, the click toasts the import's own error, as the palette command does, and the button goes away. A sidebar ctrl+v is that same click: it reads the clipboard, so it says what the button says and clears a stale button. A terminal's paste brings text that need not be the clipboard's (shift+Insert's PRIMARY, a paste history), so it leaves the button alone (round 6, ending an oscillation across rounds 2–5). *Amended 2026-11-05 (#240):* there is no click any more. A sidebar ctrl+v reads the clipboard fresh, as the palette's import does, and toasts its error when the curl has gone. A terminal's paste leaves the notice's memory alone. *Amended again 2026-11-05 (#240, review round 1):* the notice's Import is a click again, and it is that ctrl+v: it reads the clipboard fresh and toasts the import's error when the curl has gone. *Amended 2026-11-05 (#240, second review block, round 1):* the notice's Import reads no clipboard: it imports the text the notice reported (V3), so it never imports curl the notice did not speak of. A sidebar ctrl+v still reads the clipboard fresh. | mine |
| V7 | A sidebar paste that holds no curl command toasts the import's error ("clipboard doesn't hold a curl command" for ctrl+v, "the paste holds no curl command" for a terminal's paste, or the parse error). Today it is silent. | mine |
| V8 | Each button gets its label's width plus half the spare columns, a column apart. When "Import curl" will not fit, its label shortens to "curl". When even that will not fit, the row stays "+ New request" with no import button. *Amended 2026-11-05 (#240):* removed with the button. The row's layout is create folders F10 and F11. | mine; the removal mine (approved) |
| V9 | The sidebar's footer always lists `^V import curl`, last. With the setting off by default, the chip is what shows the paste; it no longer depends on the clipboard, so it needs no check. | mine |
| V10 | No button when the sidebar shows a notice (no project open), as with "+ New request". *Amended 2026-11-05 (#240):* the V3 notice is a toast, and toasts paint over dialogs and every screen, so it shows under a dialog, on the Manage screen and with no project open too. A check skipped there would leave the curl unannounced until the next focus gain (#240 review round 1). *Amended again 2026-11-05 (#240, review round 1):* the notice's controls follow it there. Under a dialog, × takes the notice down, and Import is refused with "Close the dialog first" (task #85's refusal), leaving the dialog and the notice as they were: its name prompt would open over the dialog. With no project open, Import gets the import's own "no project is open" refusal before any read, and the notice goes. On the Manage screen it imports as the palette does there. Over the edit flag's waiting screen the notice registers no controls, since that screen takes every click. A click on the notice is otherwise a click like any other control's: the click-away rules (an open cell commits, the URL line and the jq bar blur, a refused Manager or Settings input swallows it) run first. *Amended 2026-11-05 (#240, second review block, round 1):* the notice no longer shows over a dialog. You: "maybe this toast should not popup while a dialog is open? could we hold it to popup after it closes?" The checks still run wherever they run. While a dialog or the edit flag's waiting screen is up, the notice is not drawn and its clock stands still; once they are gone it slides in, with its 10 s from then. A notice already up when a dialog opens waits the same way. This replaces Import's "Close the dialog first" refusal and the waiting screen's rule above: the notice has no controls under either. A click on the notice is no click away: like the variable tooltip it floats over the panes, so an open cell, the URL line, the jq bar and a Manager or Settings input stay as they are, and a right click goes to what is under it. Import still closes the open inputs and asks the unsaved-changes question, as every import does. | mine; the hold under a dialog you (the ask), its form and the waiting screen counting as a dialog mine; no click away mine |
| V11 | The button's hover hint is "Import curl from clipboard" (the palette row's wording). *Amended 2026-11-05 (#240):* the button is gone; the sidebar's `^V import curl` footer chip keeps this hint (V9). The notice is a toast and has no hint. *Amended again 2026-11-05 (#240, review round 1):* the notice's Import has this hint, "Import curl from clipboard", and its × has "Dismiss this notice". | mine |
| V12 | The palette, `:import` and the folder menu are unchanged (the palette and `:import` still import into the space root). | mine |
| V13 | A `curl_button` setting (`config.toml`, a checkbox row "Import curl button" on the Settings tab, before "Clipboard command"), default off, turns V3–V4 on. Turning it off hides the button at once. *Amended 2026-11-05 (#240):* the setting stays opt-in and off by default, and it now turns the V3 notice on. Its `config.toml` key stays `curl_button`, so existing configs keep working (a renamed key would warn "unknown key" and turn the feature off). Only the Settings row's label changes, to "Curl import notice". Turning it off stops the checks; a notice already up fades on its timer. *Amended 2026-11-05 (#240, second review block, round 1):* turning it off takes a notice that is up, or waiting under a dialog, down at once, as it took the button down. | you (opt-in, default off; #240: "maybe an option to disable it"); the name and place mine; keeping the key and the new label mine; the notice going when turned off mine |
| V14 | No import reads the clipboard before its "no project" refusal (round 4). *Reverted 2026-10-15 (you, round 5):* an unreadable clipboard naming its cause. arboard reports an empty or image clipboard as a read error, so it said "clipboard read failed: The clipboard contents were not available…" where main says "clipboard doesn't hold a curl command"; it says that again. | mine; the revert yours |

## 3. Design

**App** owns the check. `read_clipboard_curl() -> Result<CurlBatch,
CurlError>` is the one clipboard read and parse; a clipboard that can't
be read counts as empty, which holds no curl. It
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

**Paste routing.** Two sources, two entries, one predicate.
`sidebar_takes_paste()` (no modal, Main, sidebar focused; no vim field
is live there, since `screen_field` is `None` on the sidebar) decides
both:

- ctrl+v (`Action::Paste`) *reads the clipboard*, so on the sidebar it is
  `Action::ImportCurl { folder: sidebar_import_folder() }`: the button's
  click, read for read, error for error (V6).
- A terminal's bracketed paste *delivers text*, so `paste_text` gains one
  arm, `import_pasted_curl(text)`. It comes first, ahead of the empty
  check (an empty paste there still says it holds no curl, round 8):
  `sidebar_takes_paste()` already rules out every surface the later arms
  serve (a modal, a live vim field, the response pane). It leaves the
  button alone, and "no curl" says "paste" (V6, V7).

Rounds 2–5 oscillated between making ctrl+v the button's click and
routing it with the text paste; each shape lost a distinction the other
kept. Round 6 kept both, by source.

Every import enters through `import_with(folder, read)`: the project
gate first, so nothing reads the clipboard for an import that will be
refused; then `read`; the error toast; `start_curl_import` (today's
dirty gate and prompt). `import_curl(folder)` (the palette, `:import`,
the folder menu, the button, a sidebar ctrl+v) reads with
`read_clipboard_curl`; `import_pasted_curl` parses its text.
`sidebar_import_folder()` is the one place the cursor folder (V2) is
read.

A test that pinned "a sidebar paste is unhandled"
(`bracketed_paste_routes_to_the_focused_input`) now checks the method
badge, which still takes no text.

**Click.** `Hit::SidebarImportCurl` →
`Action::ImportCurl { folder: sidebar.import_folder(space) }`, through
`update` as its siblings are.
`hint.rs` maps it to V11's caption. (The palette row's description, the
way "+ New request" gets its hint, is over the hints' 42-column limit.)

**Footer.** The sidebar's static chip list ends with
`(Key::Of(Action::Paste), "import curl", Some(Action::Paste))` (V9). The
footer functions keep their signatures. Its hover hint, on the sidebar,
is V11's "Import curl from clipboard", not the palette's "Paste
clipboard text at the cursor" (round 4). Two hover-hint layout tests
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
- V6: a click with the clipboard swapped to non-curl → error toast, flag
  cleared, no prompt. A sidebar paste of curl with a non-curl clipboard
  imports and shows no button.
- V7: a non-curl sidebar paste → the error toast.
- V9: the sidebar footer lists the chip whatever the clipboard holds;
  its hint on the sidebar is V11's.
- V14: with no project, neither an import nor a sidebar paste reads the
  clipboard for the button; an unreadable clipboard holds no curl.
- Manual: tmux, the arrows profile: the paste at the root and in a
  folder; the Settings row; the button after a copy is not expected.
- *Added 2026-11-05 (#240):* the notice's tests are in `curl_tests.rs`
  (`the_notice_*`, `the_notices_*`, `the_pointer_on_the_notice_holds_it`)
  and `components/toast.rs` (`the_curl_notice_*`, `a_held_curl_notice_*`,
  `holding_a_fading_curl_notice_restores_it`): its lifetime and the hold,
  one notice at a time, its controls and their hits, the hint dropped
  when narrow, and clicks on Import and × in the main view, under a
  dialog, with no project and over the waiting screen.
- *Added 2026-11-05 (#240, second review block, round 1):* in
  `curl_tests.rs`, `the_notice_waits_for_the_dialog_to_close`,
  `the_notice_waits_out_the_waiting_screen`,
  `the_notices_import_imports_the_curl_it_reported`,
  `a_click_on_the_notice_is_no_click_away`,
  `a_pointer_resting_where_the_notice_lands_does_not_hold_it` and the
  turn-off in `turning_the_setting_on_checks`; in `components/toast.rs`,
  `a_hidden_curl_notice_*` and
  `only_a_pointer_moved_onto_the_curl_notice_holds_it`.

## 5. Mine, for you to veto

V4's check points, V5, V6, V7, V8, V9 (the chip always shows now), V10,
V11, V12, V13's name (`curl_button`) and place (before "Clipboard
command"), V14.

*Added 2026-11-05 (#240):* one notice at a time (V3); the notice's
controls under a dialog, with no project, on Manage and over the waiting
screen, and a click on it being a click away like any control's (V10);
the hints of Import and × (V11).

*Added 2026-11-05 (#240, second review block, round 1):* Import taking
the text the notice reported and the hold on pointer motion only (V3,
V6); the hold's form under a dialog, the waiting screen counting as a
dialog, and a click on the notice being no click away (V10); the notice
going when the setting is turned off (V13). The V10 ones replace the
paragraph above's controls under a dialog and over the waiting screen,
and its click away.

## 6. Checklist

1. Spec committed (d992d43); amended after review block 1.
2. `read_clipboard_curl`, `import_with`, `check_clipboard`; the tests
   for V4–V7 and V14.
3. The paste routing and `Sidebar::import_folder`; the tests for V1, V2.
4. The sidebar split, the hit, the hint and the click; the tests for V3,
   V8, V10 and V11.
5. The footer chip; the V9 test.
6. The `curl_button` setting; the V13 tests.
7. `main.rs` launch and `on_focus_gained`; the suite, clippy and fmt;
   the tmux check.
