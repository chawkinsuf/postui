# postui

A fast, local-first terminal HTTP client. Requests live as plain TOML files
in a project folder, so they read cleanly in a diff and travel with the repo
they belong to. The whole interface works with the mouse — buttons, tabs,
and menus all click — with the keyboard as a full alternative, not a
requirement.

Requests are grouped into **spaces** — top-level folders under `requests/`
that you switch between with `ctrl+1`…`ctrl+9` (or `alt+l` / `alt+shift+l`). One
space is visible at a time and each remembers the request you had open.
Spaces, environments and variables are all edited on the Manage screen
(`alt+v`).

## Build and run

```sh
cargo build --release
./target/release/postui [directory]
```

Run with no argument to open the last project, or pass a directory to open
(or create) a project there.

## Mouse

Every control clicks: header project/space/env names open their choosers, sidebar
rows open requests, the method cell opens its dropdown, `[ Send ]` sends (and
becomes `[ Cancel ]` mid-flight), response tabs and the JSON tree arrows
toggle, and footer chips run their action. Scrollbars drag. Clicking outside
an open modal or chooser closes it, same as Esc.

Mouse capture means the terminal's own drag-to-select is off. To select text
(to copy it out with your terminal, not postui's clipboard command), hold
Shift while dragging — most terminals bypass capture for that.

### jq filter

Press `alt+q` (or click 󰈲 in the response header) to open the jq bar and filter a JSON
response live; the filter is saved with the request. Right-click any line of the tree for
verbs that write the jq for you (Filter to this, Count, Pluck field…, Where field…, Only
items where…). "Describe a filter…" sends the response's *structure* (key names and types,
never values) to the command in `ai_cmd` (default `claude -p`) and puts the reply in the bar.

## Configuration

Global settings live in `config.toml` (in your platform's config directory,
e.g. `~/.config/postui/config.toml` on Linux):

- `clipboard_cmd` — an external command to pipe copied text to (e.g.
  `"xclip -selection clipboard"`). When set, it's tried first for every
  copy. Default: unset — postui uses the OS clipboard directly, falling
  back to a terminal OSC 52 escape sequence (for SSH/headless sessions)
  when that's unavailable.
- `osc52_limit` — the largest payload, in bytes, that the OSC 52 fallback
  will send. Above this size a copy is refused rather than silently
  truncated or dumped to the terminal. Default: `65536`.
- `ai_cmd` — the command that reads a describe-a-filter prompt on stdin and
  writes the jq filter it suggests to stdout. Default: `"claude -p"`.
- `ai_confirmed` — whether the one-time confirmation to send response
  structure to `ai_cmd` has been accepted.
