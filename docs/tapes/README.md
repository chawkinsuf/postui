# Recording the README's images

The README's GIF and screenshots are recorded with
[VHS](https://github.com/charmbracelet/vhs) from the `.tape` scripts here, so
they can be made again whenever the UI changes.

## Once

1. Install VHS: `brew install vhs` (it brings `ttyd` and `ffmpeg`).
2. Install the font the tapes use, JetBrainsMono Nerd Font
   (`brew install --cask font-jetbrains-mono-nerd-font` on macOS, or from
   [nerdfonts.com](https://www.nerdfonts.com/font-downloads)). postui's icons
   need a Nerd Font.

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
and a recording config into a fresh temp directory, so a take never touches
your own config or leaves files in the repo, and every take starts from the
same state. The tapes run the release build in `target/release`.

The recording config uses the `catppuccin-mocha` theme, turns off hover hints
(VHS has no mouse to hover with), and points `ai_cmd` at `fake-ai.sh`, which
answers "Describe a filter…" with a fixed filter, so no AI account is needed
and the result is the same every time.

`curl-import.tape` pastes with VHS's `Copy` and `Paste`. If a VHS version
delivers that as typed keys rather than a terminal paste, the sidebar sees
keystrokes instead of a curl command; that's the tape to check first.
