# Configuration

postui's own settings live in one folder: `~/.config/postui/` (or
`$XDG_CONFIG_HOME/postui/` when that's set), on Linux and macOS alike. You can
write three things there, all optional:

- `config.toml`: settings;
- `keys.toml`: key bindings;
- `themes/`: custom themes.

postui writes there too: a `[projects]` table in `config.toml` (the projects
you've opened, and the last one), and `ui.toml` (which commands you use most,
for the palette's order, and the command-line history). Leave those to postui,
except one key: `root` in `[projects]` is the folder the new-project prompt and
the folder browser start in (`~/postui-projects` when unset).

```toml
[projects]
root = "~/code/apis"
```

The quickest way in is the **Settings** tab of the Manage screen (`alt+r`). Its
rows change the common settings directly, and its Edit… buttons open
`config.toml` or `keys.toml` in `$EDITOR`. If the file doesn't exist yet, the
editor starts from a commented one listing the options with their defaults
(all of them except the `[animation_ms]` table and `[projects] root`, below).
When the editor closes, postui checks what you wrote before applying it; if it
doesn't parse, you can keep editing or throw the edit away. Reset puts a file
back to the defaults, and undo brings your version back.

A setting that's missing keeps its default. One with a value of the wrong type
falls back to its default and postui says so (a bad `keymap` counts as unset,
so the first-launch question comes back). A `config.toml` that doesn't parse
at all is reported and left alone, never overwritten.

## config.toml

| Setting | Default | What it does |
|---|---|---|
| `keymap` | asked on first launch | `"arrows"` (arrows and Enter) or `"vim"`. |
| `theme` | `"terminal"` | The theme: a built-in name (below) or the file name of one in `themes/`. |
| `theme_contrast` | `0` | From `-3` (softer) to `3` (crisper); `0` is the theme as made. Also set from the theme picker. |
| `animations` | `true` | Whether transitions ease (tab underline, hover, dialogs) or jump. |
| `hover_hints` | `true` | Whether a hovered control explains itself on the footer row. |
| `curl_button` | `false` | Whether the sidebar offers an Import curl button while the clipboard holds a curl command. It reads the clipboard when postui regains focus, which on macOS can bring up the pasteboard privacy notice. |
| `jq_tab` | `"menu"` | What Tab does in the jq bar: `"menu"` lists the candidates under the bar, `"ghost"` shows the best one after the caret and cycles in place. |
| `indent` | `"spaces"` | How a request body with no indentation of its own indents: `"spaces"` or `"tabs"`. A body that's already indented keeps its own style. |
| `indent_width` | `2` | The spaces in one indent level, from 1 to 8. |
| `ai_cmd` | `"claude -p"` | The command "Describe a filter…" pipes its prompt into; it should print a jq filter. |
| `ai_confirmed` | `false` | Set by the "Always send" answer to the AI prompt's confirmation. Set it back to `false` to be asked again. |
| `clipboard_cmd` | unset | An external command to pipe copied text to, such as `"xclip -selection clipboard"`. When set, it's tried first for every copy. Otherwise postui uses the system clipboard, then the terminal's OSC 52 sequence (for SSH and headless sessions). |
| `osc52_limit` | `65536` | The largest copy, in bytes, sent over OSC 52. A bigger one is refused rather than cut short. |

Transition lengths can be tuned in an `[animation_ms]` table, each in
milliseconds: `tab_slide` (250), `hover` (70), `focus` (90), `list_travel`
(100), `modal_open` (100), `pane_collapse` (120), `toast` (100) and
`send_breathe` (700).

## keys.toml

`keys.toml` rebinds actions on top of the profile `keymap` picks. Each line
names an action and lists every key for it:

```toml
send = ["ctrl+r", "f5"]
open_jq_bar = ["alt+q"]
```

The list replaces all of the action's keys, so include the ones you want to
keep. The commented file Edit… writes lists every action that has a key, with
its current keys, which is the easiest place to start. Keys are spelled like
`ctrl+shift+e`, `alt+q`, `shift+tab`, `f9` or `esc`. `ctrl+c` can't be
rebound: it copies a selection, and quits when pressed twice. The command
palette (`ctrl+p`) shows each command's key, so a rebinding shows up there
too.

## Themes

`alt+t` opens the theme picker. `←`/`→` switch between the dark and light
lists, and `-`/`+` set the contrast. The built-ins:

- `terminal`: your terminal's own colors, read from the terminal at startup
  (the default);
- `dark`, `light`;
- `gruvbox-dark`, `gruvbox-light`;
- `catppuccin-mocha`, `catppuccin-latte`;
- `solarized-dark`, `solarized-light`.

A custom theme is six colors in `themes/<name>.toml`; postui works out every
other shade from them:

```toml
name = "Night Owl"      # optional: the label in the picker
bg = "#011627"
fg = "#d6deeb"
accent = "#82aaff"
success = "#addb67"
warning = "#ecc48d"
error = "#ef5350"
```

Set `theme = "<name>"` (the file name, without `.toml`) or pick it in the
picker. Name a pair `<base>-dark` and `<base>-light` and the picker's
dark/light switch moves between them; `counterpart = "<other name>"` pairs two
themes with any names. A custom theme with a built-in's name replaces it.
