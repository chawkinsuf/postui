# Changelog

Notable changes to postui, newest first. Versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a release may still
change the project file format.

## [Unreleased]

### Added

- At a terminal, `postui suite` shows each request as it runs, grouped by
  space, in sidebar order, and ends with a total line. `--jobs N` (`-j`)
  sends N requests at once (the default is still one at a time), and `-i`
  picks the environment, space, selector options and jobs for one run
  without saving anything. `--plain`, a pipe or `TERM=dumb` gives the plain
  text as before. The plain heading gains `jobs N` when N is more than 1,
  and the JSON gains `"jobs"`.
- A request has a Checks tab that edits its `[checks]`: the status codes
  that pass, the time limits until the first byte and until the last byte
  (the total time), and the jq checks. An empty box says what empty means
  (`no limit`; for the status `any code below 400` while another check is
  set, `run and suite: any code below 400` while none is), a new jq
  check's input says it takes a jq filter that must give true, and the
  footer says what goes in
  the input under the pointer or the keyboard cursor. The response has a
  Checks tab, when the request had checks at the send, with
  a line for each check and `✓` or `✗` on the tab for the response as a
  whole. The response shows as soon as it arrives. Until its jq checks
  have run, the tab's mark is a dim `…` and its one line reads `checking…`.
- `editor_tab_5` can be bound to open the request's Checks tab.
- Each jq check in a request's `[checks]` is stopped after 1 second and
  fails with `stopped at the 1000 ms limit`, in `postui run`, `postui
  suite` and the app alike, so a filter that never ends no longer hangs
  them. A filter in the response's jq bar (a saved view's too), `postui
  run --jq` and `--view` stop at the same limit with the same error, so
  such a filter no longer keeps a CPU core busy until postui quits. The jq
  bar's key completion stops there too and offers the keys it found before
  the stop. `jq_limit_ms` in `project.toml` sets another limit.
- A jq bar run the bar no longer needs (the filter was edited or cleared,
  a new response came, another request was opened) is stopped rather than
  left running to the limit beside the run the bar is waiting for. So is a
  key lookup for completion once another lookup, a new response or another
  request replaces it.

### Changed

- Quitting the app no longer waits for work still running in the
  background, such as a jq run, a response's checks or a cancelled send's
  DNS lookup, so it never hangs after the terminal is given back.
- `postui suite`'s total time (`totals.elapsed_ms` and the plain total) is
  now the run's wall-clock time, not the sum of the requests' times.
- A status code or a jq check listed twice in a request's `[checks]` counts
  once, in the app and in `postui run` and `postui suite`.
- The mark that a body's variables are on is part of the Body tab's label
  (`Body · vars`).
- A text input whose hover text the footer shows, the Manage screen's
  project Name and the Settings tab's text rows among them, shows it too
  when the keyboard selects it, until the mouse is used again. After a key
  or a paste, the footer no longer explains what is under the resting
  pointer.
- `ai_cmd` is now run as `exec env <ai_cmd>`, so it must be a single
  command: nothing after a `;` or `&&` in it runs, and one that starts
  with a shell builtin such as `cd` fails. To run more than one command,
  put them in a script that ends with `exec` and the AI command, and set
  `ai_cmd` to the script. Leading `NAME=value` settings still work, but
  `env` reads them: quote a `$VAR` in a setting's value, and use `$HOME`
  rather than `~`.
- A response now goes when its request goes. A request brought back by
  undoing its delete, or by redoing its create, has an empty response
  pane until it is sent again, and a send still running for a request
  that is deleted is stopped. A renamed or moved request keeps its
  response.

### Fixed

- A request sent before it was saved keeps its response when it is saved
  with a name, and a send still running for it lands on it. Before, the
  response pane went empty at the save, and the next new unsaved request
  showed that response.
- A request sent before it was saved and then discarded, or left for
  another request, no longer leaves its response, or its send still
  running, for the next new unsaved request.
- Cancelling "Describe a filter…", or quitting while it runs, stops the AI
  command. Before, only the shell that started it was stopped, so the
  command could go on running, and spending, with its reply thrown away.
  Esc stops only the program `ai_cmd` names: not the rest of a pipeline,
  and not a tool that a launcher such as `timeout` or `npx` starts.
- When a large response's Tree, or a jq filter's result, finishes loading
  while another of the response's tabs is up, that tab keeps its
  selection, cursor, scroll and search matches. A body that turns out not
  to be JSON no longer moves the view from Headers or Checks to Raw: only
  the Tree tab goes.
- On the Tree tab, while it shows the response's own tree (no filter's
  result is up), a jq filter whose result is `null` or no output, or
  switching the filter off, keeps the selection and sideways scroll, which
  it used to clear.
- On a large response, the jq bar's key completion no longer shows late
  after you open another request while a key lookup is still running. The
  lookup left behind used to throw away the open request's own lookup,
  which then started over.
- A large response opened again after visiting three other large ones keeps
  a selection made on Raw or Headers.
- Going straight back to the first of four large responses opens it at
  once, Tree tab as it was, instead of loading it again.
- A new request named like one deleted earlier no longer opens with the
  deleted request's response. Nor does a duplicate, a request renamed or
  moved to that name, or a request in a space renamed to a deleted
  space's name.

## [0.4.1] - 2026-11-13

Small improvements for variables and the project's default headers.

### Added

- The text of a `{{token}}`'s tip can be selected with the mouse (a
  double click takes a word), and ctrl+c copies it. It copies what the tip
  shows, so a hidden secret copies as dots.
- A `{{token}}`'s dialog for a declared variable's value or a selector's
  option has a Manage control (or `alt+r`) that opens that variable or
  selector in Manage.

### Changed

- The Vars tab also lists the variables in the project's default headers
  that the request sends.

### Fixed

- On the Headers tab, postui's own `Accept` or `Content-Type` is no longer
  listed, struck through, beside a project default header of the same name.

## [0.4.0] - 2026-11-12

Request folders can now be made, renamed and deleted in the sidebar, and
requests and folders moved in and out of them, by mouse or by keyboard.
postui 0.3.0 lists a folder's `.folder.toml` as a broken request, so a team
sharing a project should upgrade together.

### Added

- `+ Folder` in a wide enough sidebar (or `A`) makes a folder, and a folder's
  menu renames and deletes it. Empty folders show and stay. A folder keeps
  its name in a small `.folder.toml`. All of it undoes.
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
  `/` typed there is part of the name. A curl import's `# @name` can still
  put it in a folder.
- Manage's "Move all requests…" moves the folders too.
- `alt+↑`/`alt+↓` no longer move sidebar rows; `m` and the row menu's Move
  up and Move down do. Move to space is now `M`.
- A click on a request row opens it when the button comes up, so dragging a
  row never asks about unsaved edits.
- An undo or redo that would write over `project.toml`, `variables.toml`, an
  environment file, `.postui/secrets.toml` or a `.folder.toml` edited outside
  postui (by a `git pull`, say) no longer does so silently: that press takes
  the outside edit back, as a step of its own, with a warning, and undo and
  redo can bring it back like any other step.
- jq and `postui run --jq` treat a body with comments or NaN as not JSON,
  as the Tree tab does.

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
- A menu or dropdown on top closes when postui re-reads changed project
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
