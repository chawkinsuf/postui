#!/bin/sh
# Records the README's images again: builds the release binary, then runs
# each tape through VHS, one at a time (vhs takes one tape per call).
#
#   docs/tapes/record.sh                     every tape
#   docs/tapes/record.sh checks hero vim     only these (".tape" optional)
#
# Works from any directory. See README.md here for what VHS needs.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

if ! command -v vhs >/dev/null 2>&1; then
    echo "vhs not found; see docs/tapes/README.md" >&2
    exit 1
fi

if [ "$#" -eq 0 ]; then
    set -- docs/tapes/*.tape
fi

# Check every name before the build, so a typo fails fast.
tapes=""
for name in "$@"; do
    case "$name" in
        */*) tape="$name" ;;
        *) tape="docs/tapes/$name" ;;
    esac
    case "$tape" in
        *.tape) ;;
        *) tape="$tape.tape" ;;
    esac
    if [ ! -f "$tape" ]; then
        echo "no such tape: $tape" >&2
        exit 1
    fi
    tapes="$tapes $tape"
done

cargo build --release

for tape in $tapes; do
    echo "== $tape"
    vhs "$tape"
done
