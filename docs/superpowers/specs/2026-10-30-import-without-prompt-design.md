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
| S3 | Ignored but harmless, so no issue: output and display flags (`-s`/`--silent`, `-S`/`--show-error`, `-v`/`--verbose`, `-i`/`--include`, `-o`/`--output`, `-O`/`--remote-name`, `-J`/`--remote-header-name`, `--output-dir`, `--create-dirs`, `-w`/`--write-out`, `-D`/`--dump-header`, `-f`/`--fail`, `--fail-with-body`, `-#`/`--progress-bar`, `--no-progress-meter`, `-N`/`--no-buffer`, `--trace`, `--trace-ascii`, `--trace-time`, `--stderr`, and since review round 4 `-R`/`--remote-time`, `--remote-name-all`; `-c`/`--cookie-jar` was taken back out in round 5: it turns on curl's cookie engine, which sends a redirect's cookies on), `-L`/`--location` (postui follows redirects), `--compressed`, and a pipeline tail (`\| jq .`). Everything else the import ignores is an issue: any other flag, a redirect (`> out.json`: bash still hands curl the words after its target), assignments leading the command (`TOKEN=… curl`: curl's environment, which may be a proxy), an earlier URL a later one replaced, a dropped body, `-X` with an unknown method, a body the method doesn't send, `-u` dropped, `$(…) kept as text`. *Amended after review round 3:* see S11. | the line is you; the list is mine |
| S4 | A `# @name` already taken in its folder (by a request there, or one this paste created) brings the prompt up, prefilled with the R8 number (`List users 2`), as today. | you ("that can prompt. that is an issue the user needs to deal with") |
| S5 | In a batch, each command decides for itself. Clean named ones are created silently and only the others prompt, keeping their `(n of N)` title. One summary toast after the last command covers the whole paste, as today (§4.3). | mine (approved, chat: "yes") |
| S6 | A command with no `# @name` always prompts, as today. | you |
| S7 | The dirty gate is still asked once, before the first command, even when no command will prompt: each create opens its request and replaces the editor. | mine |
| S8 | A silent create the project refuses for any other reason (an unreadable environment file, a write that fails) toasts why, as today, and brings up that command's prompt, prefilled with the name, so the user can deal with it. A `# @name` that does not read as a request name (`sub/`) is not tried: it prompts, as a taken one does (review round 4). | mine |
| S9 | The prompt, when it shows, is unchanged: method and URL, then `ignores …` with the whole ignored list, harmless flags included. | mine |
| S10 | The summary toast is unchanged: harmless flags are still listed under `ignored:`, and still make it a warning (D3 names every dropped flag in a warning toast). | mine |
| S12 | A paste of more than 50 curl commands imports nothing: an error toast says "the paste holds N curl commands; import at most 50 at a time", before the dirty gate. Every import path (palette, sidebar paste, button, folder menu). | the limit is you ("No one should really be importing hundreds like that"); refusing the paste, at 50, is mine (approved) |
| S11 | Only the standard path skips the prompt: a paste holding any skipped line (anything but curl commands, comments and plain `NAME=value` assignments: `echo`, `alias`, `source`) prompts every command, since what such a line did is unknown, as for R7's values. A plain assignment line is read as a value only, even one curl would read itself (`export https_proxy=…`): that rare loss is accepted, and undo takes the request back. | the direction is you (after review round 3: "this was supposed to just be a simple update. i just want to keep it simple and make the standard path that doesnt need user input, not require user input"); the rule is mine |

Rounds 1–2 had grown a list of curl's environment variables and a
redirect-target tracker in the shell splitter, and round 3 found more
shell lines they misjudged. S3's redirect and leading-assignment rules
and S11 replace them, and both are removed.

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
  a body the method doesn't send, `$(…)` notes and a tail that is not a
  `| …` pipeline. `parse` sets it for leading assignments.
- The app adds S11: no silent create when the paste's `skipped_lines`
  is above zero.
- `ignored` is unchanged: still every item, in its order, for the prompt
  and the summary.

**App (`app/curl_flow.rs`).** `push_curl_prompt` is where every import
path (palette, sidebar button, sidebar paste, folder menu, a batch's
next command) asks for a name, so the choice goes there:

- If the current command is `named`, not `ignores_sent`, the paste has
  no skipped line, its name is free (`request_name_free`), and
  `curl_variables` sorts its variables with no empty secret, no `kept`
  that differs and no reserved name, `create_current` (the create
  Enter's `import_curl_create` also runs) creates it, and a loop in
  `push_curl_prompt` goes on to the next command, so a long paste of
  silent creates never nests (review round 6; round 5 had them dispatch
  the action, one `update` deep per command, which overflowed a test
  thread's stack at 20).
- Otherwise, or when that create is refused (S8), it pushes the prompt
  as today.
- The silent create can run inside `apply_modal_result`'s action loop
  (Enter on command k, then k+1's create), and a refusal's
  `last_action_failed` would push k's prompt back over k+1's, so the
  flag is taken first and put back, as `commit_settings_edit` does.

Known limit (user, after review round 6: "Leave it"): a paste that
assigns a variable a value it can't read (`export BASE=$(…)`) over a
variable the project has reads as "no news" (curl R7's kept-the-same),
so it does not stop the silent create.
- Nothing is removed. The prompt, its actions and the summary stay as
  they are.

## 4. Tests

- Core: each S3 flag (short and long) and a `| …` pipeline leave
  `ignores_sent` false; `-F`, `-d @f`, a cookie file, an unknown flag,
  two URLs, `-X PURGE`, `-u $U:p`, `$(date)` in the URL, `-X GET -d x`,
  a redirect and a leading assignment set it.
- App (`curl_tests.rs`):
  - a paste with another shell line: prompt (S11);
  - a `# @name` that reads as no name: prompt, no create tried (S8);
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
  `a_name_comment_prefills_the_prompt_and_its_folder_is_a_folder`
  became `a_name_comment_names_the_import_and_its_folder_is_a_folder`,
  which asserts the silent create for a free name and the prompt for
  taken ones.

## 5. Mine, for your veto

- S3's exact flag list. Not in it, so they still prompt: timeouts and
  retries (`-m`, `--connect-timeout`, `--retry`), HTTP version flags,
  `-x` proxy, `--resolve`, cert flags.
- Leading `TOKEN=… curl` assignments and redirects prompt (S3, after
  review round 3).
- S7: the dirty gate stays even for an all-silent import.
- S8: a refused silent create falls back to the prompt.
- S9 and S10: the prompt and the toast are unchanged, so a clean silent
  import with `-s` still ends on a warning toast `ignored: -s`. The
  other way would make harmless flags no longer warn, which changes D3.
- S11: any skipped line in the paste prompts every command.
