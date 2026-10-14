#!/bin/sh
# Points this clone at the repo's hooks (.githooks/) and at the list of
# formatting-only commits `git blame` should skip. Run once per clone.
set -e
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
git config blame.ignoreRevsFile .git-blame-ignore-revs
echo "hooks installed: .githooks/pre-commit checks cargo fmt"
