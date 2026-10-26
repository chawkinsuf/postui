# Screenshot: the theme picker with its contrast control.
# Run from the repo root: vhs docs/tapes/themes.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/themes.gif
Source docs/tapes/include/common.tape

Alt+t
Sleep 800ms
Show
Sleep 500ms
Screenshot docs/images/themes.png
# A frame after the screenshot, so the tape doesn't end on it.
Sleep 500ms
