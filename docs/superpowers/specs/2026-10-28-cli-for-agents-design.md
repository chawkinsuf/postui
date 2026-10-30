# postui on the command line, for agents — Design

Task #103. Written 2026-10-28 from a brainstorm with the user.

## 1. Intent

postui gains a command-line side whose main user is **Claude Code, through
a skill**. The user's words: "my main purpose is to enable claude code to
work with the application with a skill". The user directs; Claude:

- **creates requests** for an API it is working on, usually in a postui
  project that lives inside that API's repo and is committed with it, "can
  be used for future testing";
- **runs them** against an env (auth and variables filled in) and reads
  the response to check its own work, instead of hand-writing curl;
- **looks around** a project (envs, spaces, requests, variables,
  selectors) before adding or running anything.

The same requests serve the user for manual testing in the TUI, and as a
simple suite that runs every request in a project or a space.

### Goals

- Every command works on a registered project or on any project
  directory that is not registered (you).
- Claude edits the project's TOML files **directly**, with a command that
  verifies them (you: "while its tempting to want to go trough the app, i
  think it makes more sense to directly edit the toml, with an command to
  verify the config").
- The user and the agent never edit one project at the same time (you:
  "really any interaction of a user trying to edit a project at the same
  time as the agent is probably bad").
- A skill teaches Claude all of this, installable from the binary.

### Non-goals (for now)

- Response checks or assertions. The suite reports status and timing only
  (you: "for now status and timing i think").
- CLI commands that create or change requests, envs or variables; that is
  what direct TOML editing is for (you). `init` is the one exception (§4).
- Guarding against the user editing the TOML in their own editor while
  the agent holds the flag; nothing stops that, same as today.
- Registering a project from the CLI. Registration stays a TUI action
  (mine).
- Running requests in parallel.
- Ad-hoc requests written as a curl command line. It was on the task's
  list; it moved to its own task, #213 (you: "make it its own task").

### Anti-goals

- A read command that writes anything into the project.
- The TUI loading a half-finished set of agent edits, or saving over them.
- Secrets reaching Claude's context without the user asking for it.

## 2. What exists today (evidence)

- **Argument parsing** is hand-rolled: `config::parse_cli` →
  `CliParse::{Help, Usage, Version, Keydump, Setup, Root}`; `main.rs`
  dispatches. One optional project directory, `--debug-ai`, and the
  run-alone flags `--setup`, `--version`, `--keydump[=BITS]`.
- **Registered projects** are `ProjectsRegistry::known` in the XDG
  `config.toml`, read through `postui::config::Config`.
- **Project files:** `project.toml` (name, `spaces` order, default
  headers), `variables.toml` (declarations, selectors, shared options),
  `environments/<env>.toml`, `requests/<space>/…/<slug>.toml`. Local,
  uncommitted state lives in `.postui/` (`state.toml` with the active
  env and selector selections, `layout.toml`, `secrets.toml`, `trash/`);
  `.postui/` carries its own `.gitignore` of `*`.
- **`Project::open` writes.** It empties `.postui/trash` and, for a
  project with no environments, creates `environments/default.toml`. A
  legacy-variables project is opened with a pending migration that the
  TUI offers to run.
- **The send path** is `postui_core::prepare::prepare` (resolves
  `{{vars}}`, builds headers; `PreparedRequest` carries masked
  `display_url` and per-header `display`, `SECRET_MASK = "●●●●●●"`) then
  `postui::http::send` → `ResponseData` (status, headers, lossy-UTF-8
  body, `ttfb`, `elapsed`, `size`). `http::Clients` holds the verifying
  and the insecure client.
- **jq** is `postui_core::jq::{JqDocument::parse, run}`.
- **Reloads** are event-driven: focus-gained, Send and the choosers go
  through `Action::ReloadProjectFiles`, which re-reads when a watched
  stamp (mtime + length) changed. The request open in the editor has a
  "changed outside the app" confirm before a save over it
  (`held_request_drift`).
- **File access ownership** (CLAUDE.md): project files only through
  `Project` (`disk.rs` is the only `std::fs` in core); config through
  `Config`; four non-project sites through `postui::hostfs`.

## 3. Rules

Source: `you` = the user's own words; `mine (approved)` = my proposal the
user picked; `mine` = my call, open to veto (§8).

| # | Rule | Source |
|---|------|--------|
| R1 | The CLI's main user is Claude Code through a skill. | you |
| R2 | Commands work on registered and unregistered project directories. | you |
| R3 | Claude creates requests in a project inside the API's repo, committed with it. | you |
| R4 | Claude changes project files by editing the TOML directly; `postui check` verifies them. | you |
| R5 | An edit flag in the project directory, which the TUI knows about, marks the agent's edit. | you |
| R6 | While the flag is up, the TUI stops loading changes from disk and prevents the user from saving anything. *Amended (#216): the agent edits only a project no TUI has open, and one TUI at most has a project open (you: "require the project be closed entirely before the agent starts").* | you |
| R7 | While the flag is up, the TUI shows a blocking "updates in progress" screen with a "switch projects" option. *Amended (#216): shown when an open finds the flag up, with no project open.* | you |
| R8 | The user can override a stuck flag from the TUI. | you (need), mine (placement on the R7 screen) |
| R9 | A suite runs every request in a project, or in one space, and reports each one's status and timing. | you |
| R10 | Output can go to a file. | you |
| R11 | Secrets: Claude avoids them "unless the user explicitly asks". The CLI masks them by default; `--show-secrets` reveals. | you (rule), mine (mechanism) |
| R12 | The skill lives in the repo for manual installs, and `postui skill install` installs it into the user directory or the current directory. | you |
| R13 | Listings include file paths, so Claude knows where to edit ("with paths for config locations"). | you (task text), mine (reading) |
| R14 | *(Ad-hoc curl requests: moved to #213.)* | you |
| R15 | Built in three pieces: read and check → run → edit flag and skill. | mine (approved) |
| R16 | Read commands write nothing anywhere. | mine |
| R17 | Output is text by default; `--json` is for scripts. *(Amended 2026-10-29, #217: was JSON by default with `--text` for people. You: JSON is "harder for humans to read and it wastes tokens for agents".)* A JSON response body is given once, as `body_json` or as `body`. | you (the reason, "maybe only json body?"), mine (approved) (the flip, two keys) |
| R18 | `postui [dir]` still opens the TUI; subcommands take the first argument. | mine |
| R19 | The text output carries everything the JSON does, except `body_json` and `ok`, so the skill documents one format. `vars` lists option keys only; `vars <name>` gives one variable or selector in full. | mine (approved) (#217); `vars <name>`: you (idea), mine (approved) (positional form) |
| R20 | Text prints the project root once, near the top, and every other path in the project relative to it. A path outside the project (`-o`) stays absolute, `projects` prints absolute roots, and `--json` stays absolute. | mine (approved) (#217) |
| R21 | A value that runs over several lines (a parse error) prints on its own lines, indented under its row, so the table stays aligned. Only its first line counts toward column widths. A broken request's error goes under its row rather than in a column. | mine (approved) (#217) |
| R22 | A text-mode error is `postui: <file>: <message>` on stderr, with the file first as in `check`, and the file is absolute, since an error prints no `root` line. | mine (approved) (#217) |

## 4. Commands

Every project command takes `-p, --project <dir|name>`. A value that is
an existing directory is that directory; otherwise it is matched against
the registered projects' display names (exact, then case-insensitive;
two matches is an error naming both). Omitted, it is the current
directory. The directory must hold a `project.toml`; otherwise the
command fails with "not a postui project (no project.toml); `postui init`
makes one". (mine)

`postui <dir>` and `postui` keep opening the TUI. The first argument is
a subcommand when it is one of the names below; a directory that shares
a name opens with `postui ./list` (mine). `postui help <cmd>` and
`<cmd> --help` print usage. The parser stays hand-rolled (mine, to match
`parse_cli`; a builder who finds the subcommand tree too large for it may
propose `clap` instead, as its own step).

### Piece 1 — read and check

| Command | Output |
|---|---|
| `postui projects` | The registered projects: display name, root, whether the root still exists. Plus the config directory path. |
| `postui show [-p]` | Project name, root, and paths of `project.toml` / `variables.toml`; envs (slug, display name, file path, whether it is the TUI's saved active env); spaces (slug, display name, directory, request count); open warnings. |
| `postui requests [-p] [--space S]` | Requests in sidebar order: slug (`space/folder/leaf`), display name, method, URL as written (template, unresolved), file path. |
| `postui vars [-p] [--env E]` | For env E: every variable with description, resolved value (secrets masked), and where the value comes from (default, env, selector option, secret, missing); every selector with its fields, its options and the selection the run would use. Paths of `variables.toml` and the env file. |
| `postui check [-p] [--env E]` | Every project file parsed and validated: `project.toml`, `variables.toml`, every env file (parse + `validate_env`), every request file, `secrets.toml`, space and env names. Then, for every request, a dry `prepare` against env E (default: the env rule under piece 2) reporting unresolved variables. Errors and warnings each name the file. Exit 1 on any error. |
| `postui init <dir> [--name N]` | `Project::init` on `<dir>`: the minimum file set, never overwriting. Prints the new project's paths. Does not register it. |

**What `check` counts as an error** (mine): a file that doesn't parse or
validate, an invalid name, and a `{{name}}` that no variable or selector
field declares (likely a typo). A variable that only lacks a **selection**
or a **secret** is a warning: both live in `.postui/`, which is never
committed and which the agent never touches, so a fresh clone would
otherwise fail `check`, and `edit end` would need `--force` every time.

**Read-only open (R16).** `Project` gains a read-only open used by every
command except `init`: the same reads as `open`, a `Disk` that refuses
every write, no trash emptying, no default-env creation (an empty env
list is reported, not fixed). A project with a pending legacy-variables
migration fails with "open it in postui once to migrate it". (mine)

### Piece 2 — run

`postui run <request> [-p] [--env E] [--select SEL=OPT]... [--jq F]
[-o FILE] [--dry-run] [--show-secrets] [--timeout SECS]`

- `<request>` is the slug exactly as `postui requests` prints it. (mine)
- Prepares the request through `prepare::prepare` with the env's
  context, sends it with `http::Clients`, and prints: request slug, env,
  method, URL (masked), the sent headers (masked), status, `ttfb` and
  `elapsed` in ms, size, response headers, and the body. `--jq F` runs
  postui's jq on the body and prints the filter's output instead of the
  body (a jq error is reported, exit 1).
- `--dry-run` prints the prepared request (masked) and sends nothing.
- `-o FILE` writes the **raw response bytes** to FILE (through
  `hostfs::write_user_file`) and leaves the body out of the printed
  output. `http` gains a `send_raw` that returns the bytes;
  `send` becomes "send_raw, then the lossy decode", so the TUI is
  unchanged. With `--jq`, FILE gets the jq output instead. (mine)
- Exit 0 for a status below 400, 1 for a status of 400 or more or a
  send error, 2 for usage or project errors. (mine)
- `--timeout SECS`, default 30, `0` for none. The TUI's client has no
  timeout because its user can cancel a send. A CLI caller (Claude's
  Bash tool) can't, and one hung endpoint would stall a whole `suite`. A
  timeout is a send error. The TUI's client stays as it is. (mine)

*Amended 2026-10-29 (#215, mine):* a `--jq` error still shows
`response.body` and `body_json`, so the request needn't be sent again. A
`-o` write that fails after the send prints the report with the body and
`error` set (in `suite`, a warning), exit 1. `--select` naming one
selector twice is a usage error. And (mine (approved)) `--timeout` is not a
total deadline: the request (connect, upload and any redirects) must get
its final response started within SECS; after that each gap between reads
of the body may last SECS, so a long download that keeps coming is not cut
off. `-o` writes the file in place (not atomically), so `/dev/null` and
pipes work.

`postui suite [-p] [--space S] [--env E] [--select SEL=OPT]... [--timeout SECS] [-o FILE]`

- Runs every request in the project, or in space S, **one at a time in
  sidebar order** (later requests may rely on earlier ones). (mine)
- One result per request: slug, method, status (or the send/prepare
  error), `elapsed` ms, size. Then totals. A request that fails to
  prepare (unresolved variable) is reported and skipped; the suite goes
  on. (mine)
- `-o FILE` writes the report to FILE as well as stdout. (mine)
- Exit 1 if any request failed to prepare or send, or returned ≥ 400.

**Which values a run uses** (`run`, `suite` and `check`):

- **Env:** `--env E` (slug or display name), else the TUI's saved active
  env from `.postui/state.toml`, else the first env. The output always
  names the env used. (mine)
- **Selections:** the TUI's saved selections for that env (and the
  shared ones); each `--select SEL=OPT` overrides one for this run only,
  never written back. An unknown selector or option is a usage error
  naming the choices. (mine)
- **Secrets:** from `.postui/secrets.toml`, as the TUI uses them.

**Masking (R11).** Without `--show-secrets`, every printed string (and
the `-o` report of `suite`, but not `run`'s raw `-o` body) passes
through one mask that replaces each of the env's secret values with
`SECRET_MASK`. This covers URLs, headers, a dry-run body, and a response
that echoes a token back. Secret values shorter than 4 characters are
not masked (they would mask unrelated text); `check` warns about them.
(mine)

*Amended 2026-10-29 (#215, mine (approved); the user: "we can still adjust it if it is adding a lot of code and complication"):* the request side is masked as the TUI shows it (prepare's display forms); everything else printed has each secret's exact value replaced; a secret the server echoes back re-encoded or escaped is not caught, and the skill (piece 3) tells Claude so.

### Piece 3 — edit flag, TUI waiting screen, skill

**The flag** is `.postui/editing.toml`: `started` (RFC 3339) and an
optional `note`. It is local and uncommitted, like the rest of
`.postui/`. Its reads and writes go through a small `postui_core`
project-local API on `Disk` (like `peek_display_name`'s throwaway
`Disk`), not through `std::fs`.

| Command | Does |
|---|---|
*Amended 2026-10-30 (#216).* An agent edits only a project no postui has
open (you: "Should we just require the project be closed entirely before
the agent starts? A lock when it's opened that's removed when it's closed
or the app is closed. Maybe also prevent two instances of the tui from
opening the same project. With an override in case a lock gets stuck.").
This replaces the earlier plan here (an unsaved-work handshake before
every event, and a blocking screen over an open project), which twelve
review rounds didn't converge on; it is kept on the unmerged
`cli-edit-flag` branch.

| Command | Does |
|---|---|
| `postui edit begin [-p] [--note TEXT]` | Writes the flag. Refuses if a flag is already up (prints its age and note), and while postui has the project open. One edit at a time (you: "`--force`… its not meant to allow two things to edit at once"), so there is no `--force`. |
| `postui edit end [-p] [--force]` | Runs `check`. On errors, refuses and prints them, unless `--force`. Then removes the flag. If the flag is already gone, it says the user overrode it and exits 1. `--force` also takes down a flag that won't parse: it is for a flag "when its locked still but the user can confirm nothing is running" (you). |
| `postui edit status [-p]` | Whether a flag is up (age, note) and whether a TUI has the project open. A flag that won't parse is an error naming the file. |

**The project's lock** (you). While a TUI has a project open, it holds an
**exclusive** OS lock on `.postui/tui.lock` (`File::try_lock`, stable
since Rust 1.89; the MSRV is 1.97). Switching away or quitting releases
it, and so does a crash, so it can't get stuck. A second TUI finds it
held and doesn't open the project ("it is open in another postui
window"). Only a project is locked: a folder without a `project.toml`
gets no `.postui/` for being opened (you: a stray folder "sounds
terrible"), and postui's own create of one takes the lock first. The
command line asks with a shared try, released at once; the TUI's take
tries a few times 5 ms apart to wait one out (mine).

**The order** (mine): `edit begin` raises the flag, then asks about the
lock, and takes its flag back down if it is held; a TUI's open takes the
lock, then looks for the flag, and lets the lock go if it is up. The
opposite orders mean the two can't both go ahead.

**The waiting screen (R7, R8).** Opening a flagged project, at launch or
on a switch, shows "Updates in progress" with no project open: the
flag's age and note, and "postui opens it when the agent finishes". It
reads the flag once a second (and when focus comes back) and opens the
project when the flag comes down, fresh from its files, so there is
nothing to reload and no undo to clear. A switch onto one leaves the
current project as any switch does. The startup's create and open steps
don't run behind it.

- **Buttons**, clickable, each with a key shared by both profiles:
  **Switch projects** (you), **Override** (asks first, then removes the
  flag and opens the project; R8) and **Quit**. alt+z cycles on from it.
  A dialog the user opened over it (the chooser) comes first: the open
  waits for it to close. (mine)
- The card is sized for the widest age it shows, so it doesn't move as
  the age ticks on; the note's control characters show as codes (`^J`),
  as a field shows them. (mine)

**The skill (R12).**

- **Source:** `skills/postui/SKILL.md` at the repo root, plus a
  `reference.md` beside it: the TOML format of every project file with
  one worked example each. Both are embedded in the binary with
  `include_str!`.
- **`postui skill install [--user | --project] [--force]`:** `--user`
  (the default) writes to `~/.claude/skills/postui/`, `--project` to
  `./.claude/skills/postui/`. Identical files are left alone; files that
  differ are overwritten only with `--force`, and the command lists them
  otherwise. It writes through `hostfs`, which makes this a fifth hostfs
  site, so the CLAUDE.md list gets updated. (mine)
- **`postui skill show`:** prints `SKILL.md`, for a manual look.
- **What the skill says:**
  - when to reach for postui, and the command list;
  - **the edit protocol:** `edit begin` → edit the TOML → `check` →
    `edit end`; when `edit begin` refuses because postui has the project
    open, ask the user to switch away or quit; a stuck flag comes down
    with `edit end --force` only once the user confirms nothing is
    editing; never touch `.postui/`;
  - **secrets:** avoid them, `--show-secrets` and `.postui/secrets.toml`
    included, "unless the user explicitly asks" (you). A response body
    can still echo one back;
  - how to start a repo's request kit with `postui init`, and that the
    project gets committed.

## 5. Output format

- **JSON** (default): one JSON document on stdout per command. §4 says
  what each command reports; piece 1's plan names the keys (snake_case),
  so they're in front of the user before code. The skill documents them,
  and a later key change updates the skill in the same commit; there's
  no compatibility promise before 1.0. (mine)
  Paths are absolute. Bodies are strings; a body that parses as JSON is
  also given as `body_json` *(#217: only as `body_json`, see below)*. Errors in JSON mode print
  `{"error": "...", "file": "..."?}` on stdout and exit 2 (1 for
  `check`/run failures, which are results, not errors). (mine)
- **`--text`**: a readable layout for people. The builder designs it,
  plainly.

*Amended 2026-10-29 (#217).* Text is the default and `--json` gives the
JSON above, with the same keys and the error envelope on stdout, except
for one change to the body. A response body is given once: as
`body_json` when it parses as JSON, otherwise as the string `body`, and
the other key is `null`. Two keys, so a body that is a JSON string
(`"ok"`) can't be mistaken for the text `ok`. A JSON body loses its
exact text (whitespace, and duplicate keys, where the last one wins);
key order and numbers survive (`preserve_order`, `arbitrary_precision`),
and `-o` keeps the exact bytes. (You: "maybe only json body?" Splitting
it into two keys was my addition, and you approved it.) `--text` is gone (no compatibility promise before 1.0, and
no release has shipped the CLI). The reason is yours: JSON is "harder
for humans to read and it wastes tokens for agents". Making text the
default is my proposal, and you approved it. On the fixture project,
`vars` prints 1547 bytes of JSON against 244 bytes of text, and `show`
1329 against 986.

Because the agent now reads the text, the text has to be complete
(R19). Today it leaves out:

- in `requests`, each request's file (R13);
- in `vars`, the environment's file, `variables.toml`, and which
  selector a field belongs to;
- in `suite`, each result's method, URL and size, and every line of an
  error after the first.

`secret` and `space` aren't printed: the source column says `secret`,
and the slug starts with the space.

**`vars <name>`** (you: "just list the name and id by default with an
option to list all options and or a command for a specific var that
lists them all"; the positional form, chosen over a flag, is mine and
you approved it). `vars` lists each selector's option keys (the id
`--select` takes) on one line, with no values and no descriptions.
`vars <name>` reports one variable or selector in full. For a selector
that means its description, whether it is shared, its fields, the
selection, the file its options live in (the environment's file, or
`variables.toml` for a shared selector), and every option with its
description and values. For a
variable it means its value, source, description and selector. The
same split holds under `--json`: `vars` has no `options` values or
descriptions, and `vars <name>` has them. `variables.toml` already
refuses a name that is both a variable and a selector (`varmodel.rs`),
so a name means one thing. A name that is neither is an error naming
the choices, exit 2.

The layout (mine):

```
$ postui requests
root  /home/me/api/postui
  main/users/get-user  GET     {{base}}/users/{{user_id}}  Get user  requests/main/users/get-user.toml
  main/list            broken                                         requests/main/list.toml
    TOML parse error at line 4, column 8
      |
    4 | body = 1
  admin/x              DELETE  {{base}}/x                  X         requests/admin/x.toml

$ postui vars
root         /home/me/api/postui
environment  dev  Dev  environments/dev.toml
variables    variables.toml

variables
  base     env            https://dev  API root
  token    secret         ●●●●●●
  id       unset          -
  user_id  selector user  42

selectors
  user  selected alice  fields user_id  options alice, bob

$ postui vars user
root         /home/me/api/postui
environment  dev  Dev  environments/dev.toml
variables    variables.toml

selector     user
description  The account the requests act as
shared       no
fields       user_id
selected     alice
file         environments/dev.toml

options
  alice  user_id=42  Alice, admin
  bob    user_id=7
```

The `vars` head (root, environment, `variables.toml`) starts
`vars <name>` too, and the selector's facts are one aligned block,
`description` only when it has one. The options' file is labelled
`file`, as the JSON key is, and the options follow under their own
heading, each with its values in the selector's field order. That's
how it was built, which differs from the first sketch (mine).

- Paths are relative to the root printed at the top (R20). `projects`
  prints absolute roots, because each project has its own.
- A value that runs over several lines continues on lines indented
  under its row (R21). This is a rule of the one `table` helper.
- `run` prints the response body raw, once, after the response headers,
  and `--jq` prints its results one per line. There is no `body_json`.
  It prints `root` and the file relative to it; a `-o` file is
  absolute. `suite` prints each result's status, time, method, slug,
  URL and size, and puts an error on the lines under its row. A broken
  file's error is `<absolute file>: <message>`, and `suite` prints no
  `root`. (mine (approved))

```
$ postui suite
suite  environment dev  space (all)
  200    120 ms  GET     main/users/get-user  https://dev/users/42  512 bytes
  error          POST    main/list            https://dev/list
    {{id}} has no value
  404     35 ms  DELETE  admin/x              https://dev/x         9 bytes

3 requests, 1 passed, 2 failed, 155 ms
```
- Errors go to stderr as `postui: <file>: <message>` (R22), exit 2.
  The file is absolute: an error has no `root` line to be relative to.
  `check`'s findings are part of its report, which has a `root` line,
  so they are relative.
  Usage mistakes print the reason and the command's usage to stderr, as
  `--text` did. Exit codes are unchanged.

## 6. Layers that change

- **`postui-core`**:
  - `Project`'s read-only open;
  - `Disk`: a refuse-writes mode, plus shared and exclusive lock
    helpers (`disk.rs` stays the only `std::fs` in core);
  - the edit-flag API.
- **`postui`**:
  - a `cli/` module: the subcommand parser plus one file per piece;
  - `main.rs` dispatches the new `CliParse` variants;
  - `http::send_raw`;
  - `hostfs`: skill install;
  - `app`: the project lock taken at every open, and the waiting screen
    with its poll.
- **No change** to the request, env or variable file formats. On a
  project with no flag, the TUI's one change is the lock: a second window
  doesn't open a project the first has open.

## 7. Tests

| Rule | Test |
|---|---|
| R2 | `-p` with an unregistered tempdir project; with a registered name; with an ambiguous name; with a non-project directory. |
| R16 | Every read command on a project whose trash is non-empty and which has no env: the directory tree is byte-identical afterwards. |
| R13 | `show`/`requests`/`vars` include the right absolute file paths. |
| R4 | `check` names the file for a broken request, a broken env, an invalid selector option, and an unresolved variable; exit codes 0/1. |
| R9 | `suite` against a local test server: sidebar order, one space only, a prepare failure skipped and reported, exit 1 on a 404. |
| R10 | `run -o` writes the raw bytes of a non-UTF-8 body; `suite -o` writes the report. |
| R11 | A secret in the URL, a header, the body and an echoing response is masked; shown with `--show-secrets`; values under 4 chars are left alone and `check` warns. |
| env/select | Saved env and selections used; `--env` and `--select` override; `state.toml` unchanged afterwards. |
| R5 | `edit begin` twice refuses; `edit end` with check errors refuses, `--force` ends it; `edit end` after an override exits 1; a flag that won't parse is an error, and `edit end --force` takes it down. |
| R6/R7 | `edit begin` refuses (and leaves no flag) while a TUI holds the project's lock. A second TUI doesn't open a project the first has open. Opening a flagged project shows the screen with nothing open, and opens it fresh when the flag comes down; Switch projects, Quit and alt+z work from it. |
| R8 | Override confirms, removes the flag and opens the project. |
| check | An undeclared `{{name}}` is an error (exit 1); a missing secret or selection is a warning (exit 0). |
| timeout | `run --timeout 1` against a server that never answers is a send error, exit 1. |
| launch | Launching onto a flagged project does not call `Project::open` (no trash emptied) until the flag clears. |
| presence | The TUI's exclusive lock is seen by `edit status`; released on drop. |
| R12 | `skill install --project` into a tempdir; identical files left alone; differing files listed without `--force`, replaced with it. |
| R18 | `postui <dir>` still opens; `postui ./list` opens a dir named `list`; unknown subcommand-like words are directories. |

Tests that drive the TUI also get a tmux check of the waiting screen.

## 8. My calls, for veto

1. Subcommand names: `projects`, `show`, `requests`, `vars`, `check`,
   `init`, `run`, `suite`, `edit begin|end|status`,
   `skill install|show`.
2. `-p/--project` takes a path or a registered name, defaulting to the
   current directory.
3. ~~JSON by default, `--text` for people.~~ Text by default, `--json`
   for scripts (#217, R17).
4. Read commands never write: a read-only open, which refuses a project
   still waiting to migrate.
5. A run uses the TUI's saved env and selections unless overridden;
   overrides are never saved.
6. Exit codes: 0 fine, 1 a failed result (≥ 400, send or prepare error,
   check errors), 2 usage or project errors.
7. Suite runs one at a time in sidebar order and keeps going past
   failures.
8. *(Ad-hoc curl: moved to #213.)*
9. One output mask replaces every secret value (4+ chars) everywhere,
   response bodies included; `run -o` files are raw and unmasked.
10. `run -o` writes raw bytes via a new `http::send_raw`.
11. Flag file `.postui/editing.toml`; `edit begin` refuses a held flag;
    `edit end` refuses check errors without `--force`. *(#216: no
    `edit begin --force`; one edit at a time, you.)*
12. ~~TUI presence via a shared OS file lock, only ever reported.~~ An
    exclusive lock held while a project is open, which `edit begin`
    refuses on (#216, you).
13. ~~Blocking screen over an open project.~~ The waiting screen when
    opening a flagged project: Switch projects, Override, Quit (#216).
14. ~~The unsaved handshake.~~ Gone with #216's closed-project rule.
15. Skill at `skills/postui/` (`SKILL.md` + `reference.md`), embedded;
    `skill install` defaults to `--user` and never overwrites a differing
    file without `--force`.
16. No CLI registration of projects.
17. The parser stays hand-rolled unless it proves too large.
18. `check`: an undeclared variable is an error; a missing secret or
    selection is a warning, so `edit end` passes on a fresh clone.
19. *(Frozen work behind the screen: replaced by the handshake, 14.)*
20. ~~On release, the whole undo history is cleared.~~ Nothing is open
    behind the screen, so there is no undo to clear (#216).
21. CLI sends time out after 30 s by default (`--timeout`); the TUI's
    client is unchanged.
22. JSON key names are set in piece 1's plan; no compatibility promise
    before 1.0.

## 9. Pieces

Each piece is its own task under #103 and its own branch, merged before
the next starts. The spec is committed on `cli-features` and lands with
piece 1.

1. **Read and check:** parser and dispatch, read-only open, `projects`,
   `show`, `requests`, `vars`, `check`, `init`, JSON/text output.
2. **Run:** `run`, `suite`, `--select`, masking, `-o`,
   `http::send_raw`.
3. **Edit flag and skill:** flag API, `edit *`, the project lock, the
   waiting screen, `skills/postui/`, `skill install|show`, CLAUDE.md's
   hostfs list.
