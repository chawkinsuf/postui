# Contributing

postui is a Cargo workspace with two crates: `crates/postui-core` (projects
and their files, variables, curl and jq) and `crates/postui` (the terminal
app, including sending requests).

```sh
cargo build --release
./target/release/postui [directory]
cargo test --workspace
```

The code is formatted with `cargo fmt --all` (settings in `rustfmt.toml`;
tables meant to be read across keep their layout with `#[rustfmt::skip]`). CI
fails the build on unformatted code. Once per clone, run

```sh
scripts/install-hooks.sh
```

to install a pre-commit hook that runs the same check, and to make
`git blame` skip the formatting-only commits.

CI runs fmt, clippy and the tests on Linux and macOS. Releases are built by
dist from a version tag; the steps are in [docs/releasing.md](docs/releasing.md).
The README's screenshots are recorded from scripts in
[docs/tapes](docs/tapes/README.md).
