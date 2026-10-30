# Curl import without the name prompt (task #147)

Amends `2026-10-11-curl-import-export-design.md` §4.2 (D4, D6).

## 1. Problem

Every curl command an import takes opens the New request name prompt
(§4.2 step 3), even when the paste already names it with a `# @name`
line (D18) and nothing was lost. The prompt then asks only "is this
name right?", which the user can answer later with a rename, and the
create can be undone.

The user (task #147): "If there are no real issues with the import and
there is a name provided, don't show the confirm modal. Instead just
create the request. It can easily be undone. … skipping common
parameters that wouldn't be expected to be meaningful anyway should not
trigger the modal."

## 2. Rules

| # | Rule | Source |
|---|---|---|
| S1 | A curl command with a `# @name` line and no issue (S2) is created at once, with no prompt: the same create, undo step, "Saved X" toast and opening as an Enter on its prompt. | you (#147; chat: "if the request has a name in it already, theres no need to use the popup to ask. the user can always just rename it after") |
| S2 | An issue is any one of: something the import ignored that changes what the request sends (S3); the name taken (S4); a variable the request uses that this import declares as a secret with no value, that the project already has with a value other than the paste's, or under a reserved name (R7, the summary's warning parts). A variable the project already has, when the paste gives it no value, is no issue: the paste says nothing against what is there (the summary's plain "kept the existing X"). | you for the line ("yes, that's the line I had in mind"); the exact list is mine (approved) |
| S3 | Ignored but harmless, so no issue: output and display flags (`-s`/`--silent`, `-S`/`--show-error`, `-v`/`--verbose`, `-i`/`--include`, `-o`/`--output`, `-O`/`--remote-name`, `-J`/`--remote-header-name`, `--output-dir`, `--create-dirs`, `-w`/`--write-out`, `-D`/`--dump-header`, `-f`/`--fail`, `--fail-with-body`, `-#`/`--progress-bar`, `--no-progress-meter`, `-N`/`--no-buffer`, `--trace`, `--trace-ascii`, `--trace-time`, `--stderr`), `-L`/`--location` (postui follows redirects), `--compressed`, the pipeline tail or redirect (`\| jq .`, `> out.json`), and assignments leading the command (`TOKEN=… curl`, which set curl's environment, not what it sends). Everything else the import ignores is an issue: any other flag, an earlier URL a later one replaced, a dropped body, `-X` with an unknown method, a body the method doesn't send, `-u` dropped, `$(…) kept as text`. | the line is you; the list is mine |
| S4 | A `# @name` already taken in its folder (by a request there, or one this paste created) brings the prompt up, prefilled with the R8 number (`List users 2`), as today. | you ("that can prompt. that is an issue the user needs to deal with") |
| S5 | In a batch, each command decides for itself. Clean named ones are created silently and only the others prompt, keeping their `(n of N)` title. One summary toast after the last command covers the whole paste, as today (§4.3). | mine (approved, chat: "yes") |
| S6 | A command with no `# @name` always prompts, as today. | you |
| S7 | The dirty gate is still asked once, before the first command, even when no command will prompt: each create opens its request and replaces the editor. | mine |
| S8 | A silent create the project refuses for any other reason (an unreadable environment file, a name the create rejects) toasts why, as today, and brings up that command's prompt, prefilled with the name, so the user can deal with it. | mine |
| S9 | The prompt, when it shows, is unchanged: method and URL, then `ignores …` with the whole ignored list, harmless flags included. | mine |
| S10 | The summary toast is unchanged: harmless flags are still listed under `ignored:`, and still make it a warning (D3 names every dropped flag in a warning toast). | mine |

Skipped non-curl lines (`echo`, `cd`) are no issue of their own: they
don't change what any request sends. When they drop a variable's value
(R7 all-or-nothing), the variable is declared with no value, which is
an issue under S2. (mine)

## 3. Design

**Core (`curl/import.rs`).** The parser is the one place that still
knows which flag it ignored; once it is formatted into an `ignored`
string, telling `-s` from `-F x` means parsing the string again. So the
classification happens there:

- `CurlImport` gains `pub ignores_sent: bool`: whether anything in
  `ignored` would have changed what curl sent (S3).
- A `QUIET_FLAGS` table of long names (S3's flags). `Flags::ignore`
  sets `ignores_sent` unless the flag's long name is in it.
- `build` sets it for the dropped user, earlier URLs, an unknown `-X`,
  a body the method doesn't send and `$(…)` notes; not for the tail.
  `parse` doesn't set it for the leading assignments.
- `ignored` is unchanged: still every item, in its order, for the prompt
  and the summary.

**App (`app/curl_flow.rs`).** `push_curl_prompt` is where every import
path (palette, sidebar button, sidebar paste, folder menu, a batch's
next command) asks for a name, so the choice goes there:

- If the current command is `named`, not `ignores_sent`, its name is
  free (`free_request_name` returns it unchanged), and `curl_variables`
  sorts its variables with no empty secret, no `kept` that differs and
  no reserved name, it calls `import_curl_create(run, name)` directly,
  which advances to the next command as an Enter does.
- Otherwise, or when that create is refused (S8), it pushes the prompt
  as today. A refusal clears `last_action_failed` once its prompt is
  up: the silent create can run inside `apply_modal_result`'s action
  loop (Enter on command k, then k+1's create), and the flag left set
  would push k's prompt back over k+1's.
- Nothing is removed. The prompt, its actions and the summary stay as
  they are.

## 4. Tests

- Core: each S3 flag (short and long), a pipe tail, a redirect and a
  leading assignment leave `ignores_sent` false; `-F`, `-d @f`, a cookie
  file, an unknown flag, two URLs, `-X PURGE`, `-u $U:p`, `$(date)` in
  the URL and `-X GET -d x` set it.
- App (`curl_tests.rs`):
  - named and clean (with `-sSL --compressed | jq .`): created and opened,
    no prompt, one undo removes it (S1, S3);
  - unnamed and clean: prompt (S6);
  - named with `-F f=@a`: prompt (S2);
  - named and taken: prompt prefilled `X 2` (S4);
  - named with `$TOKEN` and no value: prompt (S2);
  - named with `$BASE` where the project's `BASE` differs: prompt (S2);
  - batch of named, named, unnamed: first two created, prompt titled
    `Import curl (3 of 3)`, one summary toast after it (S5);
  - batch whose second command reuses the first's `# @name`: prompt for
    the second, prefilled `X 2` (S4);
  - dirty editor: the gate comes first, then the silent create (S7);
  - a silent create refused by an unreadable environment file: the
    error toast and the prompt (S8);
  - Enter on an unnamed first command, then the second's silent create
    refused: only the second's prompt is up.
- Tests that pin a prompt for a free `# @name` get the new rule:
  `a_name_comment_prefills_the_prompt_and_its_folder_is_a_folder` (and
  any like it) gives its command a taken name or an issue, or asserts
  the silent create.

## 5. Mine, for your veto

- S3's exact flag list. Not in it, so they still prompt: timeouts and
  retries (`-m`, `--connect-timeout`, `--retry`), HTTP version flags,
  `-x` proxy, `--resolve`, cert flags.
- Leading `TOKEN=… curl` assignments count as harmless (S3).
- S7: the dirty gate stays even for an all-silent import.
- S8: a refused silent create falls back to the prompt.
- S9 and S10: the prompt and the toast are unchanged, so a clean silent
  import with `-s` still ends on a warning toast `ignored: -s`. The
  other way would make harmless flags no longer warn, which changes D3.
- Skipped non-curl lines alone don't prompt.
