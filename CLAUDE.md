# postui

Use the Read and Write tools so you don't trigger permissions requests with tools like sed.

Cargo workspace with two crates, both under `crates/`:

- `crates/postui-core` — project/persistence/core library
- `crates/postui` — the TUI binary
- File access has one owner per family: project files through `postui_core::project::Project`, config files through `postui::config::Config`, and the four non-project sites (export, file picker listing, editor temp file, `--debug-ai` log) through `postui::hostfs`. Only `crates/postui-core/src/disk.rs` and `crates/postui/src/hostfs.rs` may name `std::fs`; a test in each crate enforces it. Tests may write files directly (they are the outside world) and may use `postui_core::fixtures`. Design and rationale: `docs/superpowers/specs/2026-09-06-project-file-access-app-design.md`; the mechanics are in the module docs of `disk.rs` and `project/mod.rs`.

- The vim profile stays behind one seam so it can be removed whole: only `src/vim/`, `src/keyroute/` and `src/app/vim_seam.rs` may name the engine (`crate::vim::`, `engine_glue`), `App::vim`, or the seam's internals (`sync_session`, `glue_of`, `live_engine_field`, `session_parts`). The rest of the app calls the seam's `vim_*` methods, which do the whole operation or answer `None`/`false` for the plain path. `components/response.rs` and `components/editor.rs` may use the engine's code (the response's Vim window, the body's Visual and search paint). A test in `lib.rs` enforces it.

- Formatting is rustfmt's: run `cargo fmt --all` before committing (CI and the pre-commit hook in `.githooks/` both check it), and don't hand-format. A table meant to be read across opts out with `#[rustfmt::skip]` on its item, as `keyroute/table.rs`'s `TABLE` does.

There is no `postui-core` directory outside of this repo. Anything named `postui-core` lives at `crates/postui-core/` inside this repo. Everything you need is in this repo. If you think you need access to something outside of this repo, ask.
