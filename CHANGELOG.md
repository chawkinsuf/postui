# Changelog

Notable changes to postui, newest first. Versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a release may still
change the project file format.

## [0.3.0] - 2026-11-02

postui now works from the command line too, so scripts and coding agents can
read a project, send its requests and check them.

### Added

- Commands for the shell: `projects` lists the projects postui knows; `show`,
  `requests` and `vars` list what a project holds; `check` validates its
  files and variables; `init` starts one; `run` sends a request and prints
  the response; `suite` sends every request and reports status and timing.
  Output is text, or JSON with `--json`. `postui help` gives each command's
  usage and options.
- `postui edit begin` and `postui edit end` mark an agent's edit of a project,
  and `postui edit status` shows it. An edit needs the project closed in
  postui; while one is in progress, postui won't open the project and shows
  "Updates in progress" until it ends. `edit end` checks the project first;
  `--force` ends the edit anyway, for one that is no longer running.
- `postui skill install` installs a Claude Code skill that teaches an agent
  postui's commands and project files, and `postui skill show` prints it.
- Sidebar folders reorder among their siblings, by drag or `alt+↑`/`alt+↓`.
- jq filters run over every value of a streamed (NDJSON) body. An error ends
  only that value's run, as in jq.
- A pasted curl command with a `# @name` comment, and nothing to ask about,
  imports without the name prompt. Replace with curl renames the request
  from it.

### Changed

- A project opens in one postui window at a time: a second window refuses
  it with "it is open in another postui window".
- Every request asks for JSON (`Accept: application/json`), a GET included,
  unless a header or project default sets `Accept`.
- The variable tooltip waits until the pointer rests on a token for half a
  second.
- A click on an auto header or a referenced variable row moves the table
  cursor to it.

### Fixed

- Many fixes to the Variable Manager, request renames, the response pane,
  the footer and the Vim profile.
- The value dialog no longer writes an empty value when confirmed untouched.
- A curl import and Replace with curl stop while a Variable Manager or
  Settings input holds a value it can't save.

## [0.2.0] - 2026-10-27

The first public release. postui is a terminal HTTP client where everything
works by mouse or by keyboard, and requests are plain TOML files in a project
folder, so they diff cleanly and live in the repo they belong to. The
[README](https://github.com/chawkinsuf/postui#readme) covers what it does and
how to install it.
