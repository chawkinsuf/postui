# Sourced by each tape from the repo root, inside its Hide block. It copies
# the demo project and a recording config into a temp folder, so a recording
# never touches your own config or leaves files in the repo, and every take
# starts from the same state. A failed step returns non-zero, which stops the
# tape's `&& postui` from launching on a half-made copy. Its one argument is
# the key profile, arrows unless a tape asks for vim.
keymap="${1:-arrows}"
repo=$(pwd)
if [ ! -f "$repo/docs/tapes/demo/project.toml" ]; then
  echo "setup.sh: run the tapes from the repo root" >&2
  return 1
fi
# ffmpeg writes screenshots into existing folders only.
mkdir -p "$repo/docs/images" || return 1
# One folder per user, emptied on every take, so re-recording leaves nothing
# behind but the last take's copy. Built from /tmp and the numeric user id,
# not $TMPDIR, so it needs no quoting in the shell command and TOML string
# below, and two users' recordings never meet.
demo="/tmp/postui-tapes-$(id -u)"
rm -rf "$demo" && mkdir -p "$demo/config/postui" || return 1
cp -R "$repo/docs/tapes/demo/." "$demo/project" || return 1
# Open tabs and layout left by running postui on the demo folder itself
# would carry into the take.
rm -rf "$demo/project/.postui"
# A narrower sidebar than the default 28%, which leaves the demo's short
# request names a lot of empty space and the editor and response too little.
mkdir -p "$demo/project/.postui" || return 1
printf 'sidebar_width = 22\n' >"$demo/project/.postui/layout.toml" || return 1
# The stand-in AI command runs from the temp folder rather than the repo,
# whose path may hold spaces or quotes.
cp "$repo/docs/tapes/fake-ai.sh" "$demo/fake-ai.sh" || return 1
cat >"$demo/config/postui/config.toml" <<EOF || return 1
keymap = "$keymap"
theme = "catppuccin-mocha"
# The hover explanations would follow a pointer VHS never moves.
hover_hints = false
# "Describe a filter…" pipes its prompt into a stand-in (fake-ai.sh).
ai_cmd = "sh $demo/fake-ai.sh"
ai_confirmed = true
EOF
export XDG_CONFIG_HOME="$demo/config"
export POSTUI_DEMO="$demo/project"
postui() { "$repo/target/release/postui" "$@"; }
clear
return 0
