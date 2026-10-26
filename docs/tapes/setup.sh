# Sourced by each tape from the repo root, inside its Hide block. It copies
# the demo project and a recording config into a fresh temp directory, so a
# recording never touches your own config or leaves files in the repo, and
# every take starts from the same state.
repo=$(pwd)
demo=$(mktemp -d)
cp -R "$repo/docs/tapes/demo/." "$demo/project"
mkdir -p "$demo/config/postui"
cat >"$demo/config/postui/config.toml" <<EOF
keymap = "arrows"
theme = "catppuccin-mocha"
# The hover explanations would follow a pointer VHS never moves.
hover_hints = false
# "Describe a filter…" pipes its prompt into a stand-in (fake-ai.sh).
ai_cmd = "sh $repo/docs/tapes/fake-ai.sh"
ai_confirmed = true
EOF
export XDG_CONFIG_HOME="$demo/config"
export POSTUI_DEMO="$demo/project"
postui() { "$repo/target/release/postui" "$@"; }
clear
