# Using postui

Everything below works with the mouse and from the keyboard.

## Keys

The first launch asks which keys you want: arrows and Enter, or Vim. Either
way, you don't need to learn them from a page:

- the **footer** always shows the keys for whatever has focus;
- the **command palette** (`ctrl+p`) searches every command and shows its key;
- `keys.toml` changes any of them ([configuration](configuration.md)).

This guide names only a few keys: the ones you need to find everything else,
as the arrows profile spells them, and the jq bar's editing keys, which the
footer doesn't list. On macOS, `alt+` keys are `opt+`; for
`opt+` letter keys, turn on your terminal's option-as-alt setting
(`postui --setup` says how).

## Projects and spaces

A project is a folder with a `project.toml`. Run `postui <directory>` to open
one, or to create one there; with no argument postui opens the last project.
The header's project chip switches between the projects you've opened.

Requests are grouped into **spaces**: top-level folders under `requests/`.
One space shows at a time, and each remembers the request you had open; the
header's space chip switches between them. Inside a space, requests can sit in
folders, which fold. Requests can be reordered, by key or by dragging, and
moved to another space.

The **Manage** screen (`alt+r`) edits a project's variables, environments and
spaces, and has a Settings tab for postui itself.

## Editing a request

The URL line holds the method and the URL. Under it are tabs for Headers,
Params, Vars and Body:

- **Headers and Params** are tables. A row's checkbox switches it off without
  deleting it. The Headers tab also lists the headers postui adds for you,
  such as `Accept` and the project's default headers (a `[default_headers]`
  table in `project.toml`, sent with every request); the pencil on one copies
  it into the table so you can change its value. A few, like `Host`, belong to
  the HTTP client and can't be changed.
- **Vars** sets values for this request alone, and lists every variable the
  request uses with the value it has now.
- **Body** is a JSON editor that can format or minify the body, or open it in
  your `$EDITOR`.

Send sends, and turns into Cancel while the request is in flight. Saving
writes the request to its file; discarding throws unsaved changes away. A
request can also turn off TLS verification.

Everything undoes: edits, deletes, renames, moves and reorders, including ones
made in another request or on the Manage screen.

## Environments, variables and secrets

A `{{name}}` in the URL, params or headers is filled in when the request is
sent. In the body it's sent as written until you switch body substitution on
for that request. Values come from:

- **`variables.toml`**, which declares each variable, with an optional
  description and default;
- **`environments/<name>.toml`**, which sets values for one environment. The
  header's environment chip switches environments;
- **secrets**: a variable declared with `secret = true` keeps its value in
  `.postui/secrets.toml`. When postui creates `.postui/` it puts a `.gitignore`
  in it that keeps the whole folder out of git, whatever your repo's own
  `.gitignore` says; delete it if you do want `.postui/` committed. postui asks
  for a secret's value the first time a request needs it.

A **selector** bundles several variables into named options, so you can switch
them together, such as a region that sets both a host and a key. Its options
can differ per environment or be shared by all.

In the editor, hovering or clicking a `{{token}}` shows its value and where it
comes from, and selected text can be turned into a new variable.

## The response

The response pane shows the status, timing and size, with three views: Tree,
Raw and Headers. The tree folds and unfolds, the response can be searched, and
its toolbar copies the body, saves it to a file or opens it in `$EDITOR`.

### The jq filter

`alt+q` (or 󰈲 in the response header) opens the jq bar, which filters a JSON
response live as you type. The filter is saved with the request. Leaving the
bar keeps the filter on; the bar's switch turns it off without deleting it, so
the full body shows until you turn it back on. A filter that yields only
`null`, or nothing, keeps the full body on screen and says "invalid filter",
so a half-typed path doesn't blank the response you're reading. Several
outputs run together, one after another, as `jq` prints them.

As you type, the bar completes what you're most likely writing: after a `.`,
the keys the filter has reached at that point in the response; otherwise jq's
builtins (`leng` → `length`). By default the candidates are listed under the
bar: `Tab` steps through them, `shift+Tab` steps back, and `Enter` takes the
selected one. `jq_tab = "ghost"` shows the best one after the caret instead,
where `Tab` cycles and `Right` or `End` takes it
([configuration](configuration.md)).
Inside a path, `ctrl+backspace` deletes back to the previous `.`, so
`.data.items[0]` becomes `.data.` and then `.`.

**Views** save filters by name, so a request can keep several and switch
between them.

Right-click any line of the tree for filters written for you: Filter to this,
Count, Pluck field…, Where field…, Only items where…. **Describe a filter…**
(also in the palette) sends the response's *structure*, its key names and
types but never its values, to the command in `ai_cmd` (`claude -p` by
default) and puts the filter it writes in the bar. postui asks before the
first send. Filters run in jaq, the jq engine built into postui, which has no
file or process access, so a filter the AI writes can only reshape the
response on screen.

## curl

Paste a curl command into the sidebar to import it as a new request. Several
commands import one after another, and a `# @name list-users` comment above a
command names it. From the palette, a request can be copied as a curl command,
or have its contents replaced by the curl command on the clipboard.

The sidebar can also offer an Import curl button whenever the clipboard holds
a curl command. It's off by default, because offering it means reading the
clipboard unasked, which on macOS can bring up the pasteboard privacy notice.
Turn it on with `curl_button` ([configuration](configuration.md)).

## Mouse

Every control clicks: the header's chips open their choosers (the keycap pill
beside each cycles it), sidebar rows open requests, the method opens its
dropdown, Send sends, tabs switch, the JSON tree folds, and footer chips run
their action. Drag the dividers between panes to resize them, and drag
scrollbars. Clicking outside a chooser or most dialogs closes them; the few
that need an answer, like the first launch's keys question, stay. Right-click
a sidebar row, a table row or the response tree for its menu.

Mouse capture turns off your terminal's own drag-to-select. To select text for
your terminal to copy, hold `Shift` while dragging; most terminals bypass
capture for that.
