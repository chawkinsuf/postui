# Using postui

Everything below works with the mouse and from the keyboard. Keys are the
default arrows profile's; the Vim profile adds its own on top. The footer
always lists the keys for what has focus, and `ctrl+p` opens the command
palette, where every command is searchable with its key beside it. On macOS,
`alt+` keys are `opt+`.

## Projects and spaces

A project is a folder with a `project.toml`. Run `postui <directory>` to open
one, or to create one there; with no argument postui opens the last project.
The header's project chip (or `ctrl+o`) switches between the projects you've
opened, and `alt+z` / `alt+shift+z` steps through them.

Requests are grouped into **spaces**: top-level folders under `requests/`.
One space shows at a time, and each remembers the request you had open.
Switch with the header's space chip, `alt+c` / `alt+shift+c`, or
`ctrl+1`…`ctrl+9`. Inside a space, requests can sit in folders, which you can
fold, reorder (`alt+↑`/`alt+↓`, or drag) and move between spaces.

The **Manage** screen (`alt+r`) edits a project's variables, environments and
spaces, and has a Settings tab for the app itself.

## Editing a request

The editor's URL line takes the method (`alt+m` cycles it), the URL (`alt+u`)
and `{{variables}}`. Below it, tabs for Headers, Params, Vars and Body
(`alt+←`/`alt+→`):

- **Headers and Params** are tables. `alt+a` adds a row; each row has a
  checkbox to switch it off without deleting it. The Headers tab also lists
  the headers postui adds for you (an auto `Accept`, the project's default
  headers, `Host`), and the pencil on one copies it into the table so you can
  change its value.
- **Vars** sets values for this request alone, and lists every variable the
  request uses with the value it has now.
- **Body** is a JSON editor: `alt+j` formats, `alt+g` minifies, and `alt+e`
  opens the body in `$EDITOR`. `{{variables}}` in a body are sent as written
  until you switch substitution on for the request with `alt+s`.

`ctrl+r` (or `ctrl+enter`, or the Send button) sends; while it's in flight the
button becomes Cancel. `ctrl+s` saves the request to its file, and `alt+d`
discards unsaved changes. `alt+i` turns off TLS verification for the request.

Every change undoes with `ctrl+z` and redoes with `ctrl+shift+z`: edits,
deletes, renames, moves and reorders, including ones made in another request
or on the Manage screen.

## Environments, variables and secrets

A `{{name}}` in the URL, params or headers is filled in when the request is
sent, and in the body too once you switch that on (`alt+s`). Values come
from:

- **`variables.toml`**, which declares each variable, with an optional
  description and default;
- **`environments/<name>.toml`**, which sets values for one environment.
  Switch environments with the header's environment chip or
  `alt+x` / `alt+shift+x`;
- **secrets**: a variable declared with `secret = true` keeps its value in
  `.local/secrets.toml`, which the project's `.gitignore` leaves out of git.
  postui asks for a secret's value the first time a request needs it.

A **selector** bundles several variables into named options, so you can switch
them together, such as a region that sets both a host and a key. Its options
can differ per environment or be shared by all.

In the editor, `alt+v` opens the variable picker, and hovering or clicking a
`{{token}}` shows its value and where it comes from. Select text and use
`ctrl+shift+e` to turn it into a new variable.

## The response

The response pane shows the status, timing and size, with three views: Tree,
Raw and Headers. In the tree, the arrows fold and unfold. `/` searches the
response. The toolbar copies the body, saves it to a file, or opens it in
`$EDITOR`.

### The jq filter

Press `alt+q` (or click 󰈲 in the response header) to open the jq bar and filter
a JSON response live. The filter is saved with the request. `alt+q` always puts
the caret in the bar, switching the filter on if it was off. `Enter` (or
`Down`) hands focus back to the filtered tree with your edit kept; `Esc` does
the same, since the filter is live already. Inside the bar `ctrl+z` walks your
typing back, and after it closes `ctrl+z` reverts the whole edit.

`alt+shift+q` (footer chip `unfilter`, or 󰈲) is the switch: on an open bar it
closes it, which switches the filter off without deleting it, so the full body
shows until you open it again. A filter that yields only `null` (or nothing)
keeps the full body on screen and shows "invalid filter" under the bar, so a
half-typed path doesn't blank the response you're reading. Multiple outputs
run together, one after another, as `jq` prints them.

As you type, the bar completes what you're most likely writing: type `.` for
the keys the filter has reached at that point in the response, or the start of
a builtin's name (`leng` → `length`). The candidates are listed under the bar,
narrowing with each character; `Tab` picks the first, further `Tab`s step
through them, `shift+Tab` goes back, `Enter` confirms, and `Esc` un-picks.
With `jq_tab = "ghost"` the bar instead shows the best candidate after the
caret: `Tab` cycles in place and `Right` or `End` accepts. Inside a path
`ctrl+backspace` deletes back to the previous `.`, so `.data.items[0]` becomes
`.data.` and then `.`.

**Views** save filters by name (`alt+shift+j`, or the bar's `views ▾`), so a
request can keep several and switch between them.

Right-click any line of the tree for filters written for you: Filter to this,
Count, Pluck field…, Where field…, Only items where…. **Describe a filter…**
(also in the palette) sends the response's *structure*, its key names and
types but never its values, to the command in `ai_cmd` (`claude -p` by
default) and puts the filter it writes in the bar. postui asks before the
first send. The filter runs in jaq, the jq engine built into postui, which has
no file or process access, so a filter the AI writes can only reshape the
response on screen.

## curl

Paste a curl command into the sidebar (your terminal's paste, or `ctrl+v`) to
import it as a new request; several commands import one after another, and a
`# @name list-users` comment above a command names it. In the palette,
**Request: copy as curl** copies the open request as a curl command, and
**Request: replace with curl from clipboard** swaps its contents for one.

The sidebar can also offer an Import curl button whenever the clipboard holds
a curl command. It's off by default, because offering it means reading the
clipboard unasked, which on macOS can bring up the pasteboard privacy notice.
Turn it on with `curl_button` ([configuration](configuration.md)).

## Mouse

Every control clicks: the header's chips open their choosers (the keycap pill
beside each cycles it), sidebar rows open requests, the method opens its
dropdown, Send sends, tabs switch, the JSON tree folds, and footer chips run
their action. Drag the dividers between panes to resize them, and drag
scrollbars. Clicking outside an open dialog or chooser closes it, the same as
`Esc`. Right-click a sidebar row, a table row or the response tree for its
menu.

Mouse capture turns off your terminal's own drag-to-select. To select text for
your terminal to copy, hold `Shift` while dragging; most terminals bypass
capture for that.
