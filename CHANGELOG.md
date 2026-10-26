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
tabs, layout) now live in `.postui/` rather than `.local/`, and that folder
keeps itself out of git with a `.gitignore` inside it. To keep them, run this
in the project folder:

```sh
mv .local .postui && printf '*\n' > .postui/.gitignore
```

A moved folder without that `.gitignore` counts as one you chose to commit.
