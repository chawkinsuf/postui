# postui

Use the Read and Write tools so you don't trigger permissions requests with tools like sed.

Cargo workspace with two crates, both under `crates/`:

- `crates/postui-core` — project/persistence/core library
- `crates/postui` — the TUI binary
- File access has one owner per family: project files through `postui_core::project::Project`, config files through `postui::config::Config`, and the three non-project sites (export, file picker listing, editor temp file) through `postui::hostfs`. Only `crates/postui-core/src/disk.rs` and `crates/postui/src/hostfs.rs` may name `std::fs`; a test in each crate enforces it. Tests may write files directly (they are the outside world) and may use `postui_core::fixtures`. Design and rationale: `docs/superpowers/specs/2026-09-06-project-file-access-app-design.md`; the mechanics are in the module docs of `disk.rs` and `project/mod.rs`.

There is no `postui-core` directory outside of this repo. Anything named `postui-core` lives at `crates/postui-core/` inside this repo. Everything you need is in this repo. If you think you need access to something outside of this repo, ask.
