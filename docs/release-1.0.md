# 1.0 release checklist

Started 2026-10-25. Board tasks needed for 1.0 carry the `veans:1.0` label.

## Blockers on the board (`veans:1.0`)

- [x] #99 A failed landing after a project switch keeps the old project's request (merged 2026-10-25: lands on an empty scratch)
- [x] #29 `$EDITOR` round-trip drops a body's trailing newline (merged 2026-10-25, design B)
- [x] #68 A value typed on the + Add row is lost at quit (merged 2026-10-25: the value cell opens the name)
- [ ] #56 Editor Headers tab can't scroll, so header rows and project defaults can be unreachable
- [x] #97 Header strike-through carries into the method dropdown (merged 2026-10-25: a filled panel clears the page's modifiers)
- [ ] #105 Eight more palette rows with a default key show no key
- [ ] #107 Confirm dialogs' button rows overflow narrow terminals
- [ ] #116 Release workflow: cargo-dist, shell installer, Homebrew tap, Rust pinned at 1.97.1 (in progress)
- [ ] #106 Test suite fails on macOS: tests expect `alt+` where macOS shows `opt+` (CI runs tests on Linux only until then)

## Cheap fixes bundled in (all merged 2026-10-25)

- [x] #91 Palette hide/show panel commands show no key (the eight sibling rows are #105)
- [x] #92 Project switch keeps the previous project's split (already fixed by d34ea44)
- [x] #93 Launch and project switch ease into the saved layout instead of showing it
- [x] #88 Blocking dialogs' button rows overflow narrow terminals
- [x] #18 Keep the terminal-theme row (disabled) when the picker is on the other lightness

## Release pipeline

- [x] #104 CI runs fmt, clippy (all targets and shipping features) and the tests before it builds binaries (merged 2026-10-25; not yet run on GitHub — needs a push)
- [ ] Tag-triggered release workflow (`on: push: tags: ['v*']`, `permissions: contents: write`): build each target, package `postui-vX.Y.Z-<target>.tar.gz` with a sha256, create the GitHub Release. Recommended: cargo-dist (also gives a shell installer and a Homebrew tap)
- [ ] Targets: Linux x86_64 portable across glibc versions (musl, or an older runner — `ubuntu-latest` needs glibc ≥ 2.39), Linux arm64, macOS arm64, macOS x86_64
- [ ] macOS: binary is unsigned, so Gatekeeper quarantines it — notarize, or document `xattr -d com.apple.quarantine`
- [ ] Decide and document Windows support (likely: unsupported for 1.0)
- [ ] `postui --version` flag
- [ ] Bump the workspace version `0.1.0` → `1.0.0` (Cargo.lock follows)
- [ ] Cargo metadata (`description`, `license`, `repository`, `readme`) — needed only for crates.io
- [ ] CHANGELOG.md, or release notes for 1.0
- [ ] Confirm the GitHub repo is public and its description/topics are set
- [ ] Tag: `git tag -a v1.0.0 -m "postui 1.0.0"`, then `git push origin v1.0.0`

## Name

- [x] `postui` checked 2026-10-25: free on Homebrew core (formula and cask), crates.io (`postui`, `postui-core`), AUR, npm and PyPI. GitHub has ~39 small repos named postui, three of them terminal API clients (fayazara/postui 2★, marcofpadeiro/postui, rutu-sh/postui), none packaged. Not a blocker; a rename would have to happen before 1.0.

## Product checks before tagging

- [ ] Hand-check on macOS (keyboard verification is still pending)
- [ ] Review the project file format once: after 1.0, a format change needs a migration (no pre-release migrations so far)
- [ ] State the Nerd Font requirement (icons are Material glyphs)

## README rewrite

The current README is mostly a jq section, and its config list is missing most settings. Model it on Posting / Slumber (short, links out) rather than ATAC / lazygit (everything inline):

- [ ] Centered title, one-line tagline, hero GIF, link row (Install · Docs · Changelog)
- [ ] Feature bullets, then 4–5 shots: mouse-first main screen, jq filter + "Describe a filter…", Manage (environments and variables), curl import, theme picker
- [ ] Install: release binaries, `cargo install --git`, later Homebrew; Nerd Font note
- [ ] A short project-folder TOML example (plain files in git is the pitch)
- [ ] Move the key and config reference into `docs/`
- [ ] Screenshots and GIFs recorded with VHS (charmbracelet) from `.tape` scripts in the repo, so they regenerate when the UI changes; set a Nerd Font in the tape

## Housekeeping

- [ ] Ignore editor swap files (`.swp`, `.swo`, `.swn`) in `.gitignore`
- [ ] #28, #62, #98 sit in the Scrapped bucket with the done flag unset
