---
name: postui
description: Use when working with a postui project (a folder with a project.toml, often postui/ inside an API's repo) - adding or changing requests, environments, variables or selectors; sending a request to check an API you are working on instead of writing curl by hand; running a project's requests as a suite; or when the user mentions postui or wants a request kit for an API kept in its repo.
---

# postui

postui is a terminal HTTP client whose projects are plain TOML files: one
file per request, plus files for variables and environments. The user works
in its TUI; you work through its command line, and by editing the TOML
directly. The same requests then serve the user for manual testing and as a
suite.

`reference.md`, beside this file, documents every project file's TOML with
a worked example. Read it before you create or change one.

## Find the project

Every project command takes `-p <dir|name>` (a directory, or a registered
project's name) and otherwise uses the current directory.

- `postui projects`: the projects the user's postui knows, with their roots.
- `postui show`: the project's files, environments (and which one postui has
  open), spaces, and warnings.

A repo's request kit usually sits in a `postui/` folder of its own; look for
a `project.toml`.

## Look around

- `postui requests [--space S]`: every request in sidebar order, as
  `slug  method  url-as-written  name  file`. The slug (`main/orders/list`)
  is how `run` names a request: `requests/<slug>.toml`.
- `postui vars [--env E]`: every variable's value in an environment and
  where it comes from (`env`, `default`, `secret`, `selector <name>`,
  `missing_secret`, `needs_selection <selector>`, `unset`), and every
  selector with its option names and the one selected.
- `postui vars <name>`: one variable, or one selector with all its options'
  values and the file they live in.
- `postui check [--env E]`: parses and validates every file, and resolves
  every request's `{{variables}}`. An undeclared `{{name}}` is an error; a
  missing secret or selection is only a warning, since those live in the
  user's untracked `.postui/`. Exits 1 on an error.

Paths in the output are relative to the `root` line at the top. Output is
text; add `--json` only when a script needs to parse it.

## Send requests

```
postui run <slug> [--env E] [--select SELECTOR=OPTION]... [--jq F] [-o FILE] [--dry-run] [--timeout SECS]
postui suite [--space S] [--env E] [--select SELECTOR=OPTION]... [-o FILE]
```

- `run` prints the request it sent (method, URL, headers, body), then the
  status, timings, response headers and body. Read the body to check your
  work.
- `--dry-run` prints the prepared request and sends nothing: use it to check
  what a new or changed request resolves to.
- `--jq F` prints the filter's results instead of the body (postui's jq,
  jaq). `-o FILE` writes the raw body to a file instead of printing it.
- The environment is `--env`, else the one the user's postui has open, else
  the first. Selections are the user's saved ones; `--select` overrides one
  for this run only and never saves it. Pick an option name from `postui vars`.
- `suite` sends every request (or one space's) one at a time in sidebar
  order and prints each one's status, time and size, then totals. A request
  that can't be prepared is reported and skipped.
- Exit codes: 0 fine; 1 a status of 400 or more, a send or prepare error, a
  failed jq filter, or (suite) any of these; 2 a mistake in the command or
  the project.
- Sends time out after 30 s by default (`--timeout 0` for none).

`run` and `suite` write nothing into the project.

## Edit the project

You change requests, variables and environments by editing the TOML files
yourself; there are no commands that do it. Wrap every edit in the edit
flag, so the user's postui stays off the files while you work:

1. `postui edit begin --note "<what you're doing>"`. It refuses when:
   - **postui has this project open**: ask the user to switch postui to
     another project or quit it, then retry. postui won't open the project
     again until you end the edit; it shows "Updates in progress" instead.
   - **an edit is already in progress**: `postui edit status` shows since
     when and its note. If it is yours from earlier, carry on and end it.
     Otherwise ask the user; `--force` takes the flag over only once they
     confirm nothing is editing the project any more. It never lets two
     edits run at once.
2. Edit the files (`reference.md`).
3. `postui check`, and fix every error it reports. `postui run <slug>
   --dry-run` shows what a request resolves to.
4. `postui edit end`. It runs `check` again and refuses (exit 1, the flag
   kept) while there are errors; fix them. `--force` ends it anyway (still
   exit 1, since the errors are still there): only when the user asks. If
   it says **this edit is no longer in progress**, the user
   overrode your edit in postui and may have changed files since: re-read
   what you changed, tell the user, and don't assume your edit survived.

The user can then open the project in postui again. Keep the edit short:
the project can't be opened for as long as it lasts. Reading, `check` and
`run` need no flag.

Never touch `.postui/`: it holds the user's own state, selections, layout
and secrets, and is kept out of git.

## Secrets

A variable declared `secret = true` keeps its value in
`.postui/secrets.toml`, which the user fills in through postui. Avoid
secrets unless the user explicitly asks: don't read that file, don't pass
`--show-secrets`, and don't put a secret's value in a project file. Output
masks each secret's exact value as `●●●●●●`, but a value the server sends
back re-encoded or escaped (base64, JSON-escaped, URL-encoded), or that a
`--jq` filter transforms, is not caught, so a response body can still show
one. A request that needs a missing secret won't prepare: ask the user to
set it in postui.

## Start a request kit

`postui init <dir> [--name N]` makes the fewest files a project needs and
never overwrites one; it doesn't register the project with the user's
postui (they open it with `postui <dir>`). In an API's repo, put it in its
own folder (`postui/`) and commit everything but `.postui/`, which keeps
itself out of git. Then add an environment per place the API runs, declare
the variables the requests share, and add requests, all inside one edit.
