# Recording the README's images

The README's GIF and screenshots are recorded with
[VHS](https://github.com/charmbracelet/vhs) from the `.tape` scripts here, so
they can be made again whenever the UI changes.

## Once

VHS needs `ttyd` 1.7.2 or newer and `ffmpeg`, and draws the terminal in a
headless Chromium it downloads on its first run. The tapes use JetBrainsMono
Nerd Font, and it has to be installed on the system: a terminal with Nerd Font
symbols built in (Ghostty, WezTerm) doesn't lend them to VHS.

On macOS:

```sh
brew install vhs
brew install --cask font-jetbrains-mono-nerd-font
```

On Linux (x86_64), with `ffmpeg` from your package manager:

```sh
curl -L -o /tmp/vhs.deb https://github.com/charmbracelet/vhs/releases/download/v0.12.1/vhs_0.12.1_amd64.deb
sudo apt install /tmp/vhs.deb
# Distro ttyd packages can be older than VHS needs; this is the static build.
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/ttyd https://github.com/tsl0922/ttyd/releases/download/1.7.7/ttyd.x86_64
chmod +x ~/.local/bin/ttyd
mkdir -p ~/.local/share/fonts
curl -L -o /tmp/jbm.zip https://github.com/ryanoasis/nerd-fonts/releases/download/v3.5.1/JetBrainsMono.zip
unzip -o /tmp/jbm.zip -d ~/.local/share/fonts/JetBrainsMono
fc-cache -f
```

`fc-list : family | grep -i "JetBrainsMono Nerd"` should then list the font.

## Each time

From the repo root:

```sh
cargo build --release
for t in docs/tapes/*.tape; do vhs "$t"; done
```

The images land in `docs/images/`. The demo requests call
[JSONPlaceholder](https://jsonplaceholder.typicode.com), so recording needs a
network connection.

## How the tapes work

Each tape first sources `setup.sh` out of sight. It copies the `demo/` project
and a recording config into `/tmp/postui-tapes-<uid>`, emptied first, so a take never touches
your own config or leaves files in the repo, and every take starts from the
same state. The tapes run the release build in `target/release`.

The recording config uses the `catppuccin-mocha` theme, turns off hover hints
(VHS has no mouse to hover with), and points `ai_cmd` at `fake-ai.sh`, which
answers "Describe a filter…" with a fixed filter, so no AI account is needed
and the result is the same every time.

`curl-import.tape` puts its curl command on your system clipboard with VHS's
`Copy`, so recording it replaces whatever you had copied. On Linux that needs
a desktop session and a clipboard tool (`wl-clipboard` on Wayland, `xclip` or
`xsel` on X11). postui then reads
it with `ctrl+v`. VHS's `Paste` won't do here: it types the text as keys, and
the sidebar takes those as commands.

Each tape starts with `include/common.tape`: the recording settings, and the
hidden launch through `setup.sh`. `include/open-users.tape` holds the sidebar
steps into the demo's `users/` folder, the one place the tapes depend on the
demo project's layout. It sits in its own folder so the
`docs/tapes/*.tape` loop doesn't record it on its own.
