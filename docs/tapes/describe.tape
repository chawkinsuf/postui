# Screenshot: "Describe a filter…" with a request typed, over a response.
# Run from the repo root: vhs docs/tapes/describe.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/describe.gif
Source docs/tapes/include/common.tape

# Open users/list-users and send it.
Down
Sleep 300ms
Down
Sleep 300ms
Enter
Sleep 500ms
Down
Sleep 300ms
Down
Sleep 300ms
Enter
Sleep 800ms
Ctrl+R
Sleep 3s
# Find the command in the palette and describe the filter.
Ctrl+P
Sleep 500ms
Type@40ms "describe"
Sleep 500ms
Enter
Sleep 600ms
Type@40ms "names and companies"
Sleep 500ms
Show
Sleep 500ms
Screenshot docs/images/describe.png
