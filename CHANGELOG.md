# Changelog

Notable changes to postui, newest first. Versions follow
[Semantic Versioning](https://semver.org/); before 1.0, a release may still
change the project file format.

## [0.2.0] - Unreleased

The first public release. postui is a terminal HTTP client where everything
works by mouse or by keyboard, and requests are plain TOML files in a project
folder, so they diff cleanly and live in the repo they belong to. The
[README](https://github.com/chawkinsuf/postui#readme) covers what it does and
how to install it.

If you ran the 0.1.1 release candidate: a project's own files (secrets, open
tabs, layout) now live in `.postui/` rather than `.local/`, and postui no
longer reads `.local/`. To keep them, before you open the project with 0.2:

1. Make a `.postui/` folder in the project, holding a `.gitignore` that
   contains `*`. postui writes that file only when it makes the folder
   itself, and a `.postui/` without one counts as a folder you chose to
   commit.
2. Move `secrets.toml`, `state.toml` and `layout.toml` from `.local/` into
   `.postui/`.
3. Delete postui's `trash/` from `.local/`, and `.local/` itself if nothing
   else uses it. You can also delete the `/.local/` line the rc added to the
   project's `.gitignore`.
