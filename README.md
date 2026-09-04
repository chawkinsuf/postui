# postui

A fast, local-first terminal HTTP client. Requests live as plain TOML files
in a project folder, so they read cleanly in a diff and travel with the repo
they belong to. The whole interface works with the mouse — buttons, tabs,
and menus all click — with the keyboard as a full alternative, not a
requirement.

Requests are grouped into **spaces** — top-level folders under `requests/`
that you switch between with `ctrl+1`…`ctrl+9` (or `alt+c` / `alt+shift+c`). One
space is visible at a time and each remembers the request you had open.
Spaces, environments and variables are all edited on the Manage screen
(`alt+v`). Every project has at least one environment — a new one starts with
`default`, which you can rename; the last environment can't be deleted.

## Build and run

```sh
cargo build --release
./target/release/postui [directory]
```

Run with no argument to open the last project, or pass a directory to open
(or create) a project there.

## Mouse

Every control clicks: the header's project chip opens the project chooser and the
environment and space chips open their dropdowns, with the keycap pill beside each
(`alt+z` / `alt+x` / `alt+c`, shift for the other way round) cycling it, sidebar
rows open requests, the method cell opens its dropdown, `[ Send ]` sends (and
becomes `[ Cancel ]` mid-flight), response tabs and the JSON tree arrows
toggle, and footer chips run their action. Scrollbars drag. Clicking outside
an open modal or chooser closes it, same as Esc.

Mouse capture means the terminal's own drag-to-select is off. To select text
(to copy it out with your terminal, not postui's clipboard command), hold
Shift while dragging — most terminals bypass capture for that.

### jq filter

Press `alt+q` (or click 󰈲 in the response header) to open the jq bar and filter a JSON
response live; the filter is saved with the request. `alt+q` always puts the caret in the bar,
switching the filter on if it was off. `Enter` (or `Down`) hands focus back to the
filtered tree with your edit kept — what you typed, never a ghost; `Esc` cancels the edit — the filter goes back to what it was
when you started typing (and stays on), and a bar you opened onto
no filter just closes. `alt+shift+q` (footer chip `unfilter`, or 󰈲) is the switch: on an open
bar it closes it, which switches the filter off without deleting it — the full body
shows until you open it again, and the off state is saved too. A filter that yields only `null` (or nothing) keeps the full body on screen and shows
"invalid filter" under the bar, so a half-typed path doesn't blank the response you're reading. Multiple
outputs run together, one after another, as `jq` prints them. Right-click any line of the tree for
verbs that write the jq for you (Filter to this, Count, Pluck field…, Where field…, Only
items where…). "Describe a filter…" sends the response's *structure* (key names and types,
never values) to the command in `ai_cmd` (default `claude -p`) and puts the reply in the bar.
A filter the AI returns is applied immediately — safe because jaq (the embedded jq engine)
runs in-process with no file or process access, and its output never leaves the app.
As you type, the bar completes what you're most likely writing: type `.` for the keys the
filter has reached at that point in the response, or the start of a builtin's name (`leng` →
`length`). The candidates are listed under the bar as you type, narrowing with each character;
`Tab` picks the first, further `Tab`s step through them (the row slides to keep the selected one
in view), `shift+Tab` goes back, `Enter` confirms the selected one, `Esc` un-picks it, and
anything else you type keeps the selection. With `jq_tab = "cycle"` the bar instead ghosts the
best candidate after the caret: `Tab` cycles through the candidates in place, `shift+Tab` goes
back, and `Right` or `End` accepts the one showing.

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
- `jq_tab` — how the jq bar completes: `"menu"` (the default) lists the
  candidates under the bar as you type, with `Tab`/`shift+Tab` picking
  and stepping through them (`"accept"` is an older name for it);
  `"cycle"` ghosts the best candidate after the caret and steps through
  the rest in place, `Right` and `End` accepting it.
