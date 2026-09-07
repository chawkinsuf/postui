# postui

Cargo workspace with two crates, both under `crates/`:

- `crates/postui-core` — project/persistence/core library
- `crates/postui` — the TUI binary
- File access has one owner per family: project files through `postui_core::project::Project`, config files through `postui::config::Config`, and the three non-project sites (export, file picker listing, editor temp file) through `postui::hostfs`. Only `crates/postui-core/src/disk.rs` and `crates/postui/src/hostfs.rs` may name `std::fs`; a test in each crate enforces it. Tests may write files directly (they are the outside world) and may use `postui_core::fixtures`. Design and rationale: `docs/superpowers/specs/2026-09-06-project-file-access-app-design.md`; the mechanics are in the module docs of `disk.rs` and `project/mod.rs`.

There is no `postui-core` directory outside of this repo. Anything named `postui-core` lives at `crates/postui-core/` inside this repo.

## File paths

- Never type a file path from memory. Derive every path from Glob, Grep, `ls`, or `find` output first.
- Run search and read commands from the repo root using relative paths (e.g. `crates/postui-core/src/project.rs`). A wrong relative path fails with "no such file"; a wrong absolute path trips the outside-working-directory block and wastes a turn.
- If a command is blocked for reading outside the working directory, the path is almost certainly wrong. Look it up rather than retrying or asking to widen permissions.
