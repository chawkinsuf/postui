# postui

Cargo workspace with two crates, both under `crates/`:

- `crates/postui-core` — project/persistence/core library
- `crates/postui` — the TUI binary
- Project files are read and written only through `postui_core::project::Project` (and `disk::Disk` beneath it). The free functions in `project/legacy.rs`, `storage.rs`, `order.rs` and `trash.rs` are being retired; do not add callers.

There is no `postui-core` directory outside of this repo. Anything named `postui-core` lives at `crates/postui-core/` inside this repo.

## File paths

- Never type a file path from memory. Derive every path from Glob, Grep, `ls`, or `find` output first.
- Run search and read commands from the repo root using relative paths (e.g. `crates/postui-core/src/project.rs`). A wrong relative path fails with "no such file"; a wrong absolute path trips the outside-working-directory block and wastes a turn.
- If a command is blocked for reading outside the working directory, the path is almost certainly wrong. Look it up rather than retrying or asking to widen permissions.
