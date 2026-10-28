# Replace a request with curl — design

Date: 2026-10-14. Task #10. Status: reviewed in chat and approved by
the user (2026-10-14), `mine` choices included.

## 1. Problem

A request kept in step with a "master" curl command (a README, devtools'
Copy as cURL) today has to be re-imported as a new request and the old
one deleted, losing its name, place in the list, jq filter and request
variables. The task asks for a way to update an existing request from a
curl command.

What exists (spec `2026-10-11-curl-import-export-design.md`):
`curl::parse` gives a `CurlBatch` of `CurlImport`s; `import_curl_create`
splits each import's variables into new secrets, names the project
already has (`kept`) and reserved names, then
`Project::create_request_with_secrets` creates the request and declares
the secrets as one journal entry; `CurlImportRun::summary` builds the
toast.

Two facts shape the design:

- A request save is not journaled. Editor edits are app-history
  `EditorDelta` steps, memory only (`project/requests.rs:205`).
- Declaring a variable is a journaled `Project` step. Extract-to-variable
  already pairs one with an editor change. It records the declaration as
  a `Project` step **first**, then lets the editor change be the next
  `capture_undo`'s own `EditorDelta`. That is two undo steps, ordered so
  that undoing never reloads the request over unsaved work
  (`app.rs:6976`).

## 2. Rules

| # | Behaviour | From |
|---|---|---|
| U1 | The curl's request replaces the open request **in the editor**, as unsaved edits: the request shows dirty; Save writes it; Discard or undo reverts it. | you |
| U2 | Replaced: method, URL (query verbatim, curl R2), params (cleared: the query rides in the URL), headers, body, `substitute_body`, `insecure`. Kept: name, slug (folder, position), jq filter and its on/off, request variables. **Amended by #148 (§6):** a `# @name` renames. | you |
| U3 | Shell variables go through import's pipeline unchanged (curl R7, D15–D17): a `$NAME` the project doesn't have is declared a secret, with the paste's literal value in the active environment when it has one; names the project has are kept as they are; reserved names are not saved. | you |
| U4 | A clipboard with more than one curl command refuses: error toast `clipboard holds 3 curl commands; replace takes one`. Nothing changes. | you |
| U5 | Undo: the declarations are one `Project` step and the editor change is the next one, an `EditorDelta`, as extract-to-variable does. The first undo puts the request back exactly as it was (unsaved edits included); the second removes the declarations. Discard reverts the request and leaves the declarations. Replace without new declarations is one step. | you (accepted with "same as import") |
| U6 | Entry points: request row right-click **"Replace with curl"** directly after "Copy as curl" (not on broken rows); palette **"Request: replace with curl from clipboard"** for what the editor holds, scratch included. No new key, no new vim ex verb: vim's `:` line already reaches palette rows. | mine (as curl R5) |
| U7 | Replacing a row that is not open opens it first. Unsaved work in the editor meets the usual dirty gate (`open`), then the replace runs. Everything that can refuse runs **before** the gate: the clipboard read, the parse, reading the row's file, and sorting the variables (an unreadable environment file). A refusal only toasts, and the gate's follow-on carries what was read, so the clipboard is never read twice (first review round, 2026-10-14). | mine |
| U8 | Replacing the open request needs no gate: its unsaved edits are replaced in place and undo brings them back (U5). No confirm (delete-never-confirms: undo is the net). | mine |
| U9 | ~~`# @name` (D18) is ignored: replace never renames.~~ **Reversed by #148 (§6, N1–N6).** | mine |
| U10 | A curl variable named like one of the request's own **enabled** variables counts as kept: the request variable wins at send, so nothing is declared over it. A disabled one sends nothing, so the name is sorted as if it were absent (first review round). | mine |
| U11 | Toast: `Replaced {name} from curl`, then import's summary parts (§4.3: ignored flags, converted, saved, declared with no value, kept, reserved, values dropped, skipped lines) after `; `, then the undo hint (` — ^Z undoes`). A scratch is named `the unsaved request`. Its kind is the summary's (warning or success); success when the summary is empty. A curl the request already matches reads `Nothing to replace: {name} already matches the curl`, with the undo hint only when it declared something (third review round). | mine |
| U12 | Errors as import: unreadable clipboard, `CurlError`, no project, and a refused declaration (an unreadable environment file) each toast and change nothing. | mine |

## 3. Design

**Core** (`varedit_ops.rs`): split the declaring half of
`create_request_with_secrets` into `pub fn declare_secrets(&mut self,
secrets)`. It runs the same validation (valid name, not reserved, not
`variable_exists`) and is one journal entry, `declare variables`.
`create_request_with_secrets` becomes create plus the shared
declare body inside its one transaction. Nothing else in core changes.

**App**:

- `Action::ReplaceWithCurl(Option<String>)`: `None` is what the editor
  holds, as with `CopyAsCurl`. Then `Action::ForceReplaceWithCurl { slug,
  replace }`, the dirty gate's follow-on: open that slug, then replace with
  the `CurlReplace` (the batch and its sorted variables) read before the
  gate.
- `curl_flow.rs`: `replace_with_curl`. It reads and parses the clipboard
  (`clipboard_curl`, shared with import); refuses U4; commits an open
  cell, flushes an open field and reloads the project files, as Copy as
  curl does (third review round); sorts the variables
  against the target's own variables (U10); then gates, opens, or applies
  (U7). `apply_curl_replace`:
  1. flushes the open field session, then reads the fields it keeps.
     Import's partition is one helper, `curl_variables(&import,
     request_vars)`, used by both flows. An unreadable environment file is
     its error.
  2. When there are secrets: `begin_op`, then `declare_secrets`, then
     `record_project_step` (label: change to {request}), recorded before
     the editor change, as extract does.
  3. The new request is the curl's, with U2's kept fields from
     `editor.current_request()`. `no_coalesce`, then
     `editor.apply_snapshot`, then `sync_active_tab`. The next
     `capture_undo` records the `EditorDelta`.
  4. The toast (U11). It reuses `CurlImportRun::summary` over a
     one-command run with one `CreatedImport`.
- `palette.rs`: the command row after "Request: copy as curl".
  `app.rs`: the menu row after "Copy as curl".

No contract changes beyond the new actions. `StepKind`, `View`,
`after_replay` and `land` are untouched. A replace of a row not open yet
opens it and replaces it in one input event, and a capture that sees the
slug change only re-seeds, so `open_and_replace` seats the opened request
as the undo shadow itself, and the replace is a step of its own. (The
second review round moved that into `land`'s open for every request; the
fourth found it stopped recording a Discard that reopens the open
request, so it is back in the one caller, mine.)

## 4. Tests

Core: `declare_secrets` declares, saves values, drops a leftover
`secrets.toml` value, refuses a taken or reserved name, and one undo
removes it all. `create_request_with_secrets`' existing tests stay
green.

App (`curl_tests.rs`):
- U1/U2: replace the open request. The editor holds the curl's
  method/URL/headers/body/insecure; params are empty; name, jq and
  request variables are kept; the request is dirty; the file is
  unchanged until Save.
- U5: with a new secret, undo #1 restores the editor exactly, including
  earlier unsaved edits, while the secret stays; undo #2 removes it.
  Without one, a single undo restores.
- U4: two commands refuse, with the editor and history untouched. A
  non-curl clipboard toasts.
- U7: from the row menu of a non-open row with a dirty editor: the gate,
  then Discard opens the row and replaces it. A clean editor goes
  straight through.
- U10: a request-variable name is kept, not declared.
- U11: toast text with the undo hint and a summary part.
- U6: the menu row sits after Copy as curl and is absent on broken rows;
  the palette row is present.

Manual: tmux, both profiles.

## 5. My choices, for veto

U6 (entry points, no ex verb), U7 (open-then-replace with a parse before
the gate), U8 (no gate on the open request), U9 (no rename from
`# @name`), U10 (request variable counts as kept), U11 (toast wording),
U12.

## 6. Amendment 2026-10-28 (task #148): `# @name` renames

Task #148: "Replace request with curl does not replace name". The user
picked the rule from my options, so it is `mine (approved)`: replace
renames the request only when the curl text carries a `# @name X`
comment (D18), read as import reads it (spaces folded, an empty comment
falls back to no name). A bare curl keeps the name, as before. This
reverses U9 and the "name kept" part of U2.

Today a rename (`Action::RenameRequest`, `Project::rename_request`) is
immediate and journaled: the file moves to the new name's slug, its
`name` is rewritten, a taken name is refused, and the editor follows in
place with no reload. The editor's `name` is never an unsaved edit: a
rename and its undo set the live and saved copies together. So the
replace's rename goes through that path rather than landing in the
editor as an unsaved name (which would show nowhere until Save, write
the name under the old file name, skip the taken-name check, and be
reset by a rename's undo).

| # | Behaviour | From |
|---|---|---|
| N1 | Replace runs the existing rename first, as its own journaled project step, then lands the editor change as U1 says. The order of steps is declarations (U5), rename, editor change, so undo takes back the editor change, then the rename, then the declarations. Discard reverts the fields and keeps the rename; undo takes it back. A rename that still fails when it runs (a file written while the gate was up) toasts rename's error and the replace lands under the old name, so the declarations are never left without the request that uses them (mine, first review round). | mine (approved) |
| N2 | The rename's checks (an empty leaf, a name another request in the target folder answers to) run up front with U7's other refusals, before the dirty gate. A refusal toasts rename's own words after U12's `cannot replace: ` (`cannot replace: a request named "X" already exists here`, first review round) and changes nothing. | mine (approved) |
| N3 | **Amended 2026-10-28 (#148, fifth review round):** a name with a `/` in it reads from the space's top, as the Rename prompt and a top-level import read it, so replaying the paste that imported a request lands where the import put it (`# @name api/X` on `api/ping` lands at `api/X`, not `api/api/X`). A leading `/` counts as one (`/X` lands at the top, as the Rename prompt reads it, mine). A bare name keeps the request in its folder. All through rename's folder move, inside the request's space. ~~First: a `/` read relative to the request's own folder (`# @name sub/X` on `api/ping` landed at `api/sub/X`); the round argued that nested a replayed import's folder twice.~~ | mine (approved) |
| N4 | A scratch request ignores `# @name`: it has no file to rename, and Save still asks for a name. | mine (approved) |
| N5 | A `# @name` that is the request's own name, spelled the same, in its own folder, renames nothing, and is asked before the rename's checks, so it never refuses. Another case is a rename (`Kept` to `KEPT` rewrites the name and keeps the file), as the Rename prompt takes a case-only change. (First written as "ignoring case"; two review rounds argued a case-only change asked for in the comment was dropped silently, mine.) | mine |
| N6 | Toast: `Replaced {old} from curl, renamed to {new}`, then U11's summary parts and undo hint. A rename with no field change still reads `Replaced …` (it changed something). | mine |

Design: core `CurlImport` holds `named: Option<String>`, the `# @name`
text alone, set only from the comment; import's `suggested_name()` is
`named`, else the URL's suggestion (one copy of the name, third review
round). `Project::check_rename_request` is `rename_request`'s checks
without the write, and the replace asks it before the gate. `CurlReplace`
carries the checked rename target. `apply_curl_replace` runs the rename
through `App::rename_request`, the method the `RenameRequest` arm now
calls, and then reads the editor for the kept fields, so the name kept
is the new one.

The rename re-keys the undo shadow (its slug and name), as the rename's
undo and redo already did in `after_replay`. Without it a capture that
sees the slug change only re-seeds, so the replace's editor change in
the same event would not be recorded, and a rename that keeps the slug
records the name as an editor edit whose undo puts the old name back
over the file's (a Rename prompt bug on main too, `Kept` to `Kept!`).
Only the key moves, never the shadow's contents, so no pending edit is
swallowed: unlike #10's seat in `land`, which overwrote the shadow and
stopped a Discard being recorded. (Rounds 1–2 seated the shadow in the
replace alone; two review rounds argued for the shared shape, mine.)

Tests (`curl_tests.rs`): a `# @name` renames the open request and a row
not yet open; a folder in the name reads from the space's top (`api/X` from a request in
`api/` lands at `api/X`; `/X` at the top) and a bare name keeps the folder;
a taken name refuses before the gate with nothing changed; the same name
renames nothing and another case renames; a rename refused after the
gate still replaces; a scratch ignores it; a bare curl keeps
the name; undo takes back the editor change, then the rename, then the
declarations; Discard keeps the rename. Core: `named` is set only from
the comment.
