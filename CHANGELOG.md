# Changelog

Notable changes to postui, newest first. Versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a release may still
change the project file format.

## [0.4.0] - 2026-11-12

Request folders can now be made, renamed and deleted in the sidebar, and
requests and folders moved in and out of them, by mouse or by keyboard.
postui 0.3.0 lists a folder's `.folder.toml` as a broken request, so a team
sharing a project should upgrade together.

### Added

- `+ Folder` in the sidebar (or `A`) makes a folder, and a folder's menu
  renames and deletes it. Empty folders show and stay. A folder keeps its
  name in a small `.folder.toml`. All of it undoes.
- Requests and folders drag into and out of folders. A closed folder opens
  when the drag rests on it, and where several folders end on one row, the
  pointer's column picks the level.
- A keyboard move: `m` picks up a sidebar row, the arrows step it through the
  places a drag would take it, Enter drops it and Esc puts it back.
- jq filters and the Tree tab read a server-sent events body (as streamed AI
  replies use) one event at a time, as they do NDJSON.

### Changed

- The opt-in curl import button (`curl_button`) is now a notice in the
  corner, with an Import button, the paste key and a close. It shows once per
  command, and not for curl that postui copied itself.
- Requests and folders share one order in each folder, so they can be mixed.
  `postui requests` and `postui suite` follow it.
- A new request or folder usually goes right after the selected row; "New
  request here…" and "New folder here…" put it last in that folder. A curl
  import, or a request moved to another space, goes last.
- The New request prompt and a request's Rename prompt take a name only: a
  `/` typed there is part of the name. A curl import's `# @name` can still put it in a folder.
- Manage's "Move all requests…" moves the folders too.
- `alt+↑`/`alt+↓` no longer move sidebar rows; `m` and the row menu's Move
  up and Move down do. Move to space is now `M`.
- A click on a request row opens it when the button comes up, so dragging a
  row never asks about unsaved edits.
- An undo or redo that reaches `project.toml`, `variables.toml`, an
  environment file or a `.folder.toml` changed outside postui (by a
  `git pull`, say) no longer writes over it silently: that press takes the
  outside change back, as a step of its own, with a warning. The next press
  goes on to your own steps, and the other key (redo after an undo, undo
  after a redo) brings the outside change back.
- jq, `postui run --jq` and the suite treat a body with comments or NaN as
  not JSON, as the Tree tab does.

### Fixed

- Many fixes for project files changed outside postui: reordering the
  sidebar or the environments, and creating, renaming or deleting an
  environment, no longer write over an order changed outside; creating or
  reordering spaces re-reads the project first, so a space directory made by
  hand shows with its requests.
- Undoing or redoing a request rename changes only the name, so later edits
  to the request stay.
- A body nested deeper than 256 levels no longer crashes postui when jq reads
  it.
- After a lost mouse release, the next click no longer starts a drag.
- An open menu or dropdown closes when postui re-reads changed project
  files, so a row menu can't act on whatever took its row.
- A crash after a trip to `$EDITOR` restores the terminal once, not once per
  trip.

## [0.3.0] - 2026-11-02

postui now works from the command line too, so scripts and coding agents can
read a project, send its requests and check them.

### Added

- Commands for the shell: `projects` lists the projects postui knows; `show`,
  `requests` and `vars` list what a project holds; `check` validates its
  files and variables; `init` starts one; `run` sends a request and prints
  the response; `suite` sends every request and reports status and timing.
  Output is text; most commands print JSON with `--json`. `postui help`
  gives each command's usage and options.
- `postui edit begin` and `postui edit end` mark an agent's edit of a project,
  and `postui edit status` shows it. An edit needs the project closed in
  postui; while one is in progress, postui won't open the project and shows
  "Updates in progress" until it ends. `edit end` checks the project first;
  `--force` ends the edit anyway, for one that is no longer running.
- `postui skill install` installs a Claude Code skill that teaches an agent
  postui's commands and project files, and `postui skill show` prints it.
- Sidebar folders reorder among their siblings, by drag or `alt+↑`/`alt+↓`.
  Their order is saved with the requests' in the space's `order` list in
  `project.toml`, as `"name/"` entries.
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
- A curl import takes at most 50 commands at a time.

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
