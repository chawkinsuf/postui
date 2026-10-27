# Screenshot: the filter "Describe a filter…" wrote (fake-ai.sh's), applied
# to the response.
# Run from the repo root: vhs docs/tapes/describe.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/describe.gif
Source docs/tapes/include/common.tape

# Open users/list-users and send it.
Source docs/tapes/include/open-users.tape
Down
Sleep 300ms
Down
Sleep 300ms
Enter
Sleep 800ms
Ctrl+R
# The status line: "200", then the time ("158 ms", or "1.3 s" past a second).
Wait+Screen@15s /200 +\d/
Sleep 500ms
# Find the command in the palette and describe the filter.
Ctrl+P
Sleep 500ms
Type@40ms "describe"
Sleep 500ms
Enter
Sleep 600ms
Type@40ms "names and companies"
Sleep 500ms
# The filter lands in the jq bar, applied.
Enter
Wait+Screen@15s /jq map\(/
Sleep 800ms
Show
Sleep 500ms
Screenshot docs/images/describe.png
# A frame after the screenshot, so the tape doesn't end on it.
Sleep 500ms
