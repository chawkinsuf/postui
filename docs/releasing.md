# Releasing postui

Releases are built and published by [dist](https://axodotdev.github.io/cargo-dist/book/)
(formerly cargo-dist). Pushing a version tag such as `v1.0.0` runs
`.github/workflows/release.yml`, which:

1. runs the same fmt, clippy and test gates as every push to main
   (`.github/workflows/checks.yml`), alongside the builds;
2. builds `postui` for four targets: `aarch64-apple-darwin` and
   `x86_64-apple-darwin` (both on the arm64 macOS runner),
   `x86_64-unknown-linux-gnu` (Ubuntu 22.04, so glibc 2.35 or newer) and
   `aarch64-unknown-linux-gnu` (GitHub's arm64 Ubuntu 22.04 runner);
3. only if the gates and every build passed, creates the GitHub Release with
   a `.tar.xz` and `.sha256` per target, a shell installer
   (`postui-installer.sh`) and a source tarball;
4. pushes the Homebrew formula to the tap `chawkinsuf/homebrew-postui`.

Windows isn't built, and nothing is published to crates.io.

Users then install with either of:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/chawkinsuf/postui/releases/latest/download/postui-installer.sh | sh
brew install chawkinsuf/postui/postui
```

The shell installer puts `postui` in `$CARGO_HOME/bin` (`~/.cargo/bin` by
default) and adds that to `PATH`.

The configuration is `dist-workspace.toml`. `release.yml` is generated from
it: never edit `release.yml` by hand. dist refuses to plan a release whose
`release.yml` differs from what it would generate (`dist plan` shows the
same error locally), so a hand edit stops the release in its first job.

## One-time setup

Done once, by hand, before the first release:

1. **Make the repository public** (`chawkinsuf/postui`), if it isn't. GitHub's
   arm64 Linux runner (`ubuntu-22.04-arm`) is free only for public
   repositories; in a private one the `aarch64-unknown-linux-gnu` build
   never gets a runner.
2. **Create the tap repository** `chawkinsuf/homebrew-postui` on GitHub:
   public (Homebrew clones it without credentials), with a README so it has
   a default branch. dist manages everything else in it (`Formula/postui.rb`).
3. **Create a token that can push to the tap.** Either:
   - a fine-grained personal access token: resource owner `chawkinsuf`,
     repository access "Only select repositories" → `homebrew-postui`,
     repository permission **Contents: Read and write**; or
   - a classic personal access token with the `repo` scope (what dist's
     docs describe).

   Note its expiry date: when it expires, the Homebrew step fails (the
   GitHub Release itself is still published) until a new token is stored.
4. **Store it as a secret** on `chawkinsuf/postui`: Settings → Secrets and
   variables → Actions → New repository secret, named exactly
   `HOMEBREW_TAP_TOKEN`.

The GitHub Release itself needs no secret: `release.yml` asks for
`contents: write` on the workflow's own token.

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml` (both crates
   take it from there), then run `cargo check` so `Cargo.lock` picks up the
   new version. CI builds with `--locked`, so the lock file must be
   committed with it.
2. Commit both, merge to main as usual and push main. The `build` workflow
   runs the gates on it.
3. Tag the merge commit and push the tag:

   ```sh
   git tag -a v1.2.3 -m "postui 1.2.3"
   git push origin v1.2.3
   ```

   The tag must be `v` plus exactly the version in `Cargo.toml`; dist
   refuses a tag that doesn't match.
4. Watch the `Release` workflow on GitHub. If a gate or a build fails,
   nothing is published. Fix it on main, then either move the tag
   (`git tag -d v1.2.3`, `git push origin :refs/tags/v1.2.3`, tag again) or
   release the next patch version.

   If the GitHub Release was created but a later job failed (most likely
   `publish-homebrew-formula`, with an expired or missing
   `HOMEBREW_TAP_TOKEN`), don't move the tag: the release already exists,
   so a new run fails creating it. Fix the cause (store a new token) and
   use "Re-run failed jobs" on that workflow run.

Before tagging, `dist plan` (run from the repository root) shows what the
release will contain.

### Prereleases

A version with a prerelease suffix (`1.2.3-rc.1`, tagged `v1.2.3-rc.1`)
goes through the same workflow. It is published as a GitHub *prerelease*,
and the Homebrew formula is **not** updated, so `brew` users stay on the
last full release. This is how to get downloadable binaries of main
between releases: there is no per-push build.

## Bumping the pinned Rust toolchain

`rust-toolchain.toml` pins the exact Rust release for local builds, the
`build` workflow and the release builds alike. rustup installs it on the
first cargo call in the repo; where it can't download (offline, or a
read-only rustup home such as a sandbox), run `rustup toolchain install`
once from the repository root first. `rust-version` in `Cargo.toml` is its
second copy, kept at the same minor version.

1. Set `channel` in `rust-toolchain.toml` to the new release (for example
   `"1.98.0"`), and `rust-version` under `[workspace.package]` in
   `Cargo.toml` to its minor version (`"1.98"`).
2. Run `rustup toolchain install` (it reads the file), then
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` and `cargo fmt --all --check`. Fix whatever the
   new release's lints or rustfmt flag in the same branch.
3. Commit; the next push to main checks it on GitHub.

## Upgrading dist

dist's GitHub releases are ahead of crates.io (0.33.x is on GitHub only),
so install it from there:

1. Install the new version into `~/.cargo/bin`, from
   <https://github.com/axodotdev/cargo-dist/releases> (download the archive
   for your platform and check its `.sha256`, or run its
   `cargo-dist-installer.sh`). `dist --version` confirms it.
2. From the repository root, run `dist init`. It updates
   `cargo-dist-version` in `dist-workspace.toml` and regenerates
   `release.yml`; accept the current answers. (Alternatively edit
   `cargo-dist-version` by hand and run `dist generate`.)
3. Check the result: `dist generate --check` prints nothing,
   `dist plan` still lists the four targets and both installers, and the
   diff of `release.yml` is dist's own.
4. Commit `dist-workspace.toml` and `release.yml` together.

The release workflow downloads exactly the `cargo-dist-version` named in
`dist-workspace.toml`, so CI and the local `dist` should match.
