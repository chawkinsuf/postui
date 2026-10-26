# Screenshot: the Manage screen with a variable's description and its value in
# the active environment.
# Run from the repo root: vhs docs/tapes/manage.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/manage.gif
Source docs/tapes/include/common.tape

Alt+r
Sleep 700ms
# Select base_url, the first variable.
Down
Sleep 700ms
Show
Sleep 500ms
Screenshot docs/images/manage.png
