# 0.2 release checklist

Started 2026-10-25 as the 1.0 checklist. The first public release is 0.2.0 instead, with 1.0 after real-world use (the user, 2026-10-25). Board tasks needed for 0.2 carry the `veans:0.2` label (renamed from `veans:1.0`).

## Blockers on the board (`veans:0.2`)

- [x] #99 A failed landing after a project switch keeps the old project's request (merged 2026-10-25: lands on an empty scratch)
- [x] #29 `$EDITOR` round-trip drops a body's trailing newline (merged 2026-10-25, design B)
- [x] #68 A value typed on the + Add row is lost at quit (merged 2026-10-25: the value cell opens the name)
- [x] #56 Editor Headers tab can't scroll, so header rows and project defaults can be unreachable (squash-merged 2026-10-26; full history tag `headers-tab-scroll-history`)
- [ ] #121 Undo of a row add doesn't put the table cursor back where it was (found during #56; pre-existing)
- [x] #97 Header strike-through carries into the method dropdown (merged 2026-10-25: a filled panel clears the page's modifiers)
- [ ] #105 Eight more palette rows with a default key show no key
- [ ] #107 Confirm dialogs' button rows overflow narrow terminals
- [x] #115 The vim body search prompt shows the row's leftover text behind the query (merged 2026-10-25: the row is blanked first)
- [x] #116 Release workflow: cargo-dist, shell installer, Homebrew tap, Rust pinned at 1.97.1 (merged 2026-10-25)
- [x] #124 Replacing or deleting a body selection that ends a line adds a blank line after it (merged 2026-10-25: edtui's delete, with the break after the selection kept)
- [x] #126 The views dropdown's rename and delete buttons are small targets (merged 2026-10-26: a cell of padding each side, part of the button)
- [ ] #106 Test suite fails on macOS: tests expect `alt+` where macOS shows `opt+` (CI runs tests on Linux only until then)

## Cheap fixes bundled in (all merged 2026-10-25)

- [x] #91 Palette hide/show panel commands show no key (the eight sibling rows are #105)
- [x] #92 Project switch keeps the previous project's split (already fixed by d34ea44)
- [x] #93 Launch and project switch ease into the saved layout instead of showing it
- [x] #88 Blocking dialogs' button rows overflow narrow terminals
- [x] #18 Keep the terminal-theme row (disabled) when the picker is on the other lightness
- [x] #117 The editor tab underline has one mover (a project switch no longer starts a stray glide)

## Release pipeline

- [x] #104 CI runs fmt, clippy (all targets and shipping features) and the tests before it builds binaries
- [x] #116 Tag-triggered release with dist: shared checks, four targets (macOS arm64/x86_64, Linux x86_64/arm64; Linux on Ubuntu 22.04 for glibc 2.35), shell installer into `~/.local/bin`, Homebrew formula to `chawkinsuf/homebrew-postui`; steps in `docs/releasing.md`
- [x] Rust pinned at 1.97.1 (`rust-toolchain.toml`), `rust-version = "1.97"`
- [x] Cargo metadata (`description`, `license`, `repository`, `homepage`)
- [x] The GitHub repo is public; the tap repo `chawkinsuf/homebrew-postui` exists
- [x] `HOMEBREW_TAP_TOKEN` secret on chawkinsuf/postui (set by the user 2026-10-25; first used by the first full release)
- [x] #120 Prerelease `v0.1.1-rc.1`: CI and the whole release ran green on GitHub (Homebrew skipped as a prerelease); the installer installs and runs
- [ ] macOS: binary is unsigned. The shell installer and Homebrew avoid the quarantine flag; a browser-downloaded tarball needs `xattr -d com.apple.quarantine` (document it)
- [ ] Decide and document Windows support (likely: unsupported for 0.2)
- [ ] `postui --version` flag
- [ ] Bump the workspace version to `0.2.0` (Cargo.lock follows)
- [ ] CHANGELOG.md, or release notes for 0.2
- [ ] The repo's GitHub description and topics
- [ ] Tag: `git tag -a v0.2.0 -m "postui 0.2.0"`, then `git push origin v0.2.0`

## Name

- [x] `postui` checked 2026-10-25: free on Homebrew core (formula and cask), crates.io (`postui`, `postui-core`), AUR, npm and PyPI. GitHub has ~39 small repos named postui, three of them terminal API clients (fayazara/postui 2★, marcofpadeiro/postui, rutu-sh/postui), none packaged. Not a blocker; a rename is cheapest before the first release.

## Product checks before tagging

- [ ] Hand-check on macOS (keyboard verification is still pending)
- [ ] State the Nerd Font requirement (icons are Material glyphs)

## Before 1.0 (not 0.2)

- [ ] Review the project file format once: from 1.0 on, a format change needs a migration; 0.x releases still don't

## README rewrite

The current README is mostly a jq section, and its config list is missing most settings. Model it on Posting / Slumber (short, links out) rather than ATAC / lazygit (everything inline):

- [ ] Centered title, one-line tagline, hero GIF, link row (Install · Docs · Changelog)
- [ ] Feature bullets, then 4–5 shots: mouse-first main screen, jq filter + "Describe a filter…", Manage (environments and variables), curl import, theme picker
- [ ] Install: the shell installer, Homebrew (`brew install chawkinsuf/postui/postui`), release binaries, `cargo install --git`; Nerd Font note
- [ ] A short project-folder TOML example (plain files in git is the pitch)
- [ ] Move the key and config reference into `docs/`
- [ ] Screenshots and GIFs recorded with VHS (charmbracelet) from `.tape` scripts in the repo, so they regenerate when the UI changes; set a Nerd Font in the tape

## Housekeeping

- [ ] Ignore editor swap files (`.swp`, `.swo`, `.swn`) in `.gitignore`
- [ ] #28, #62, #98 sit in the Scrapped bucket with the done flag unset
