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
| U2 | Replaced: method, URL (query verbatim, curl R2), params (cleared: the query rides in the URL), headers, body, `substitute_body`, `insecure`. Kept: name, slug (folder, position), jq filter and its on/off, request variables. | you |
| U3 | Shell variables go through import's pipeline unchanged (curl R7, D15–D17): a `$NAME` the project doesn't have is declared a secret, with the paste's literal value in the active environment when it has one; names the project has are kept as they are; reserved names are not saved. | you |
| U4 | A clipboard with more than one curl command refuses: error toast `clipboard holds 3 curl commands; replace takes one`. Nothing changes. | you |
| U5 | Undo: the declarations are one `Project` step and the editor change is the next one, an `EditorDelta`, as extract-to-variable does. The first undo puts the request back exactly as it was (unsaved edits included); the second removes the declarations. Discard reverts the request and leaves the declarations. Replace without new declarations is one step. | you (accepted with "same as import") |
| U6 | Entry points: request row right-click **"Replace with curl"** directly after "Copy as curl" (not on broken rows); palette **"Request: replace with curl from clipboard"** for what the editor holds, scratch included. No new key, no new vim ex verb: vim's `:` line already reaches palette rows. | mine (as curl R5) |
| U7 | Replacing a row that is not open opens it first. Unsaved work in the editor meets the usual dirty gate (`open`), then the replace runs. The clipboard is parsed **before** the gate, so a non-curl clipboard just toasts. | mine |
| U8 | Replacing the open request needs no gate: its unsaved edits are replaced in place and undo brings them back (U5). No confirm (delete-never-confirms: undo is the net). | mine |
| U9 | `# @name` (D18) is ignored: replace never renames. | mine |
| U10 | A curl variable named like one of the request's own variables counts as kept: the request variable wins at send, so nothing is declared over it. | mine |
| U11 | Toast: `Replaced {name} from curl`, then import's summary parts (§4.3: ignored flags, converted, saved, declared with no value, kept, reserved, values dropped, skipped lines) after `; `, then the undo hint (` — ^Z undoes`). A scratch is named `the unsaved request`. Its kind is the summary's (warning or success); success when the summary is empty. | mine |
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
  holds, as with `CopyAsCurl`. Then `Action::ForceReplaceWithCurl(String)`,
  the dirty gate's follow-on: open that slug, then replace.
- `curl_flow.rs`: `replace_with_curl`. It reads the clipboard and parses;
  refuses U4; opens or gates for U7; then runs `apply_curl_replace`:
  1. partitions the variables. Import's partition moves into one helper,
     `curl_variables(&import, request_vars)`, used by both flows; U10
     passes the open request's variables.
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
`after_replay` and `land` are untouched.

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
