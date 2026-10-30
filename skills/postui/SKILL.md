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
  every request's `{{variables}}`. An undeclared `{{name}}` is an error, and
  so is a URL using a variable whose value holds a `{{name}}` (values aren't
  filled in, so postui won't send it); any value holding one is a warning on
  its file, and so is a missing secret or selection, since those live in the
  user's untracked `.postui/`. Exits 1 on an error.

Paths in the output are relative to the `root` line at the top. Output is
text; add `--json` only when a script needs to parse it.

## Send requests

```
postui run <slug> [--env E] [--select SELECTOR=OPTION]... [--var NAME=VALUE]...
                  [--jq F | --view[=NAME]] [-o FILE] [-i | -v] [--dry-run] [--timeout SECS]
postui suite [--space S] [--env E] [--select SELECTOR=OPTION]... [--var NAME=VALUE]...
             [-o FILE] [--timeout SECS]
```

- `run` prints the response body and nothing else, as curl does. A status
  of 400 or more (`postui: 404 Not Found`), an error and any warning go to
  stderr, one `postui:` line each. `-i` prints the status line and response
  headers before the body; `-v` prints the whole run: the request as sent
  (method, URL, headers, body), the status, timings and response headers,
  then the body.
- `--dry-run` prints the prepared request and sends nothing: use it to check
  what a new or changed request resolves to.
- `--jq F` prints the filter's results instead of the body, one a line
  (postui's jq, jaq). `--view` uses the request's own filter, the one its
  response pane applies, and `--view=NAME` one of its saved `[[jq_view]]`s:
  reach for these first when a request has them, to keep a large body out
  of your context. A filter that fails prints its error and no body.
- `-o FILE` writes the raw body to a file instead of printing it; use it
  for a binary body, which prints as text. With `--jq` or `--view` the
  results still print and the file gets the body as text, masked like
  printed output: add it when you try a filter on a body you can't just
  fetch again (a large one, a POST that changes something, a model's
  answer), so that if the filter fails the body is in the file to read.
- The environment is `--env`, else the one the user's postui has open, else
  the first. Selections are the user's saved ones; `--select` overrides one
  for this run only and never saves it. Pick an option name from `postui vars`.
- `--var NAME=VALUE` gives a variable a value for this run only, over every
  other source, the request's own `[variables]` included. The name must be
  one the request could use: declared in `variables.toml`, set by the
  environment, or in the request's `[variables]`.
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

Each refusal below exits 1 with the reason on stderr; a mistake in the
command or the project exits 2.

1. `postui check` first, so you know which errors were there before you
   changed anything.
2. `postui edit begin --note "<what you're doing>"`. It refuses when:
   - **postui has this project open**: ask the user to switch postui to
     another project or quit it, then retry. postui won't open the project
     again until you end the edit; it shows "Updates in progress" instead.
   - **an edit is already in progress**: `postui edit status` shows since
     when and its note. If it is yours from earlier, carry on and end it.
     Otherwise ask the user. Only once they confirm nothing is editing the
     project any more, `postui edit end --force` takes the flag down and you
     can begin. Never run two edits at once.
3. Edit the files (`reference.md`).
4. `postui check`, and fix every error your changes caused. `postui run
   <slug> --dry-run` shows what a request resolves to.
5. `postui edit end`. It runs `check` again and refuses (exit 1, the flag
   kept) while there are errors. Fix yours. If it still refuses, ask the
   user what to do about the rest, an error that was there before your edit
   among them: fix it, or end the edit anyway with `postui edit end --force`
   (still exit 1, since the errors are still there). Never `--force`
   without asking. If it says **no edit is in progress**, the user
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
one. A request that needs a missing secret won't prepare (`check` only
warns about it). When what you are checking doesn't need the secret, such
as a header the server ignores, `--var NAME=` sends the request with it
empty for that run; otherwise ask the user to set it in postui. Never give
a secret's real value with `--var`.

## Start a request kit

`postui init <dir> [--name N]` makes the fewest files a project needs and
never overwrites one; it doesn't register the project with the user's
postui (they open it with `postui <dir>`). In an API's repo, put it in its
own folder (`postui/`) and commit everything but `.postui/`, which keeps
itself out of git. Then add an environment per place the API runs, declare
the variables the requests share, and add requests, all inside one edit.
