# 0.2 release checklist

Started 2026-10-25 as the 1.0 checklist. The first public release is 0.2.0 instead, with 1.0 after real-world use (the user, 2026-10-25). Board tasks needed for 0.2 carry the `veans:0.2` label (renamed from `veans:1.0`).

## Blockers on the board (`veans:0.2`)

- [x] #99 A failed landing after a project switch keeps the old project's request (merged 2026-10-25: lands on an empty scratch)
- [x] #29 `$EDITOR` round-trip drops a body's trailing newline (merged 2026-10-25, design B)
- [x] #68 A value typed on the + Add row is lost at quit (merged 2026-10-25: the value cell opens the name)
- [x] #56 Editor Headers tab can't scroll, so header rows and project defaults can be unreachable (squash-merged 2026-10-26; full history tag `headers-tab-scroll-history`)
- [x] #121 Undo of a row add doesn't put the table cursor back where it was (merged 2026-10-26: undo steps record the table cursor; an add through + Add undoes onto + Add)
- [x] #97 Header strike-through carries into the method dropdown (merged 2026-10-25: a filled panel clears the page's modifiers)
- [x] #105 Eight more palette rows with a default key show no key (merged 2026-10-26: a row's key is looked up by its action)
- [x] #107 Confirm dialogs' button rows overflow narrow terminals (merged 2026-10-26: Confirm uses the blocking dialogs' wrapping layout)
- [x] #115 The vim body search prompt shows the row's leftover text behind the query (merged 2026-10-25: the row is blanked first)
- [x] #116 Release workflow: cargo-dist, shell installer, Homebrew tap, Rust pinned at 1.97.1 (merged 2026-10-25)
- [x] #124 Replacing or deleting a body selection that ends a line adds a blank line after it (merged 2026-10-25: edtui's delete, with the break after the selection kept)
- [x] #126 The views dropdown's rename and delete buttons are small targets (merged 2026-10-26: a cell of padding each side, part of the button)
- [x] #106 Test suite fails on macOS: tests expect `alt+` where macOS shows `opt+` (merged 2026-10-26: CI runs the tests on macOS too)
- [x] #135 No way to reach a URL/body variable's popup from the keyboard (pulled in by the user, 2026-10-26; merged 2026-10-26: alt+o/n/h act on the caret's tooltip)
- [x] #136 The main header rows have no `y` copy key, which the auto rows have (merged 2026-10-26 with #137)
- [x] #137 Copy a whole header (`Name: value`) (merged 2026-10-26: `Y`, vim's `"+yy`, shift+click on a copy pill where the terminal passes it on; #159 moved the click to alt)
- [x] #151 The variable edit dialog changes request vars under an open cell (pulled in by the user, 2026-10-26, with the four below; merged 2026-10-26: the dialog commits an open Vars cell as it opens)
- [x] #144 A bad command-line argument prints usage and exits 0 (merged 2026-10-26: `postui: <reason>` and the usage on stderr, exit 2; `--help`/`-h` on stdout)
- [x] #145 Variable Manager grid: arrows copy key is `c` while every other list copies with `y` (merged 2026-10-26: `y` copies, `p` pastes, as on every list)
- [x] #128 Arrows body keyboard selections can't include a line break (merged 2026-10-27: selections take the breaks they cross; shift+Down takes the line and its break, the user's call)
- [x] #127 `ctrl+y` in the arrows body pastes back the last deleted selection (merged with #156: the body drops edtui's private paste, undo, redo and search keys)
- [x] #156 `ctrl+u` in the arrows body runs edtui's own one-character undo (pulled in by the user with #127)
- [x] #159 A copy pill's shift+click never arrives, even in Ghostty (filed by the user, 2026-10-26; merged 2026-10-27: the whole-header click is alt+click, `opt+click` on macOS)
- [x] #166 A shift+arrow after a click selects from the click point, not the caret (pulled in by the user, 2026-10-27; merged 2026-10-27: with no live selection, the caret is the anchor)
- [ ] #157 `ctrl+j` in the arrows body breaks the line without auto-indent (pulled in by the user, 2026-10-27, with the two below)
- [ ] #161 A non-UTF-8 command-line argument panics
- [ ] #162 Printing `--version`, `--help` or `--setup` panics when stdout can't be written (e.g. `postui --help | head`)

## Cheap fixes bundled in (all merged 2026-10-25)

- [x] #91 Palette hide/show panel commands show no key (the eight sibling rows are #105)
- [x] #92 Project switch keeps the previous project's split (already fixed by d34ea44)
- [x] #93 Launch and project switch ease into the saved layout instead of showing it
- [x] #88 Blocking dialogs' button rows overflow narrow terminals
- [x] #18 Keep the terminal-theme row (disabled) when the picker is on the other lightness
- [x] #117 The editor tab underline has one mover (a project switch no longer starts a stray glide)

## Release pipeline

- [x] #104 CI runs fmt, clippy (all targets and shipping features) and the tests before it builds binaries
- [x] #116 Tag-triggered release with dist: shared checks, four targets (macOS arm64/x86_64, Linux x86_64/arm64; Linux on Ubuntu 22.04 for glibc 2.35), shell installer into `~/.local/bin`, Homebrew formula to `chawkinsuf/homebrew-tap` (#155 renamed it from `homebrew-postui`); steps in `docs/releasing.md`
- [x] Rust pinned at 1.97.1 (`rust-toolchain.toml`), `rust-version = "1.97"`
- [x] Cargo metadata (`description`, `license`, `repository`, `homepage`)
- [x] The GitHub repo is public
- [x] #155 The tap is `chawkinsuf/homebrew-tap`, shared so other apps can publish to it (the user, 2026-10-26); install is `brew install chawkinsuf/tap/postui`. The user renamed the GitHub repo from `homebrew-postui` (2026-10-26)
- [x] `HOMEBREW_TAP_TOKEN` secret on chawkinsuf/postui (set by the user 2026-10-25; first used by the first full release)
- [x] #120 Prerelease `v0.1.1-rc.1`: CI and the whole release ran green on GitHub (Homebrew skipped as a prerelease); the installer installs and runs
- [x] macOS: the binary is unsigned. Homebrew (and the shell installer) avoid the quarantine flag, so the README makes Homebrew the recommended install (the user, 2026-10-26: "just make brew the recommended way to install"); done in #138
- [x] Windows: not supported natively for 0.2; WSL 2 is recommended (the user, 2026-10-26). The README says so (#138)
- [x] #139 `postui --version` flag (also `-V`; prints `postui <version>`, in `--help`)
- [x] #140 CHANGELOG.md: one short first-release entry (the user, 2026-10-26: "go with the tiny changelog"). dist puts the matching section on the GitHub Release as its notes and takes the title from the heading, so #142 dates it
- [ ] #141 The repo's GitHub description and topics
- [ ] #142 Last (the user, 2026-10-26): bump the workspace version to `0.2.0` (Cargo.lock follows), then tag: `git tag -a v0.2.0 -m "postui 0.2.0"`, then `git push origin v0.2.0`

## Name

- [x] `postui` checked 2026-10-25: free on Homebrew core (formula and cask), crates.io (`postui`, `postui-core`), AUR, npm and PyPI. GitHub has ~39 small repos named postui, three of them terminal API clients (fayazara/postui 2★, marcofpadeiro/postui, rutu-sh/postui), none packaged. Not a blocker; a rename is cheapest before the first release.

## Product checks before tagging

- [x] Hand-check on macOS (the user, 2026-10-26: "i have already tested on a mac")
- [x] State the Nerd Font requirement (icons are Material glyphs): the README's Install section, which also names terminals with the symbols built in (#138)

## Before 1.0 (not 0.2)

- [ ] Review the project file format once: from 1.0 on, a format change needs a migration; 0.x releases still don't

## README rewrite (#138)

The last big item (the user, 2026-10-26). The current README is mostly a jq section, and its config list is missing most settings. Model it on Posting / Slumber (short, links out) rather than ATAC / lazygit (everything inline):

- [x] Centered title, one-line tagline, hero GIF, link row (Install · Guide · Configuration · Changelog). Tagline: "A terminal HTTP client where everything works by mouse or by keyboard" (mine (approved); the user ruled out a mouse-first framing)
- [x] Feature bullets, then 4 shots under the hero: Manage, curl import, theme picker, "Describe a filter…"
- [x] Install: Homebrew first, as the recommended way (`brew install chawkinsuf/tap/postui`, since #155), then the shell installer, release binaries, `cargo install --git`; Nerd Font note; Windows users: WSL 2
- [x] A short project-folder TOML example (plain files in git is the pitch)
- [x] Config reference in `docs/configuration.md`, concepts in `docs/guide.md`. There is no key reference on purpose: the footer and the `ctrl+p` palette show each profile's live keys, which a page can't (mine (approved), review of #138)
- [x] Screenshots and GIFs recorded with VHS (charmbracelet) from `docs/tapes/*.tape`, so they regenerate when the UI changes; JetBrainsMono Nerd Font

## Housekeeping

- [ ] Ignore editor swap files (`.swp`, `.swo`, `.swn`) in `.gitignore`
- [ ] #28, #62, #98 sit in the Scrapped bucket with the done flag unset
