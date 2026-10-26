# Screenshot: "Describe a filter…" with a request typed, over a response.
# Run from the repo root: vhs docs/tapes/describe.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/describe.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 1400
Set Height 860
Set Padding 12
Set Theme "Catppuccin Mocha"
Set TypingSpeed 40ms

Hide
Type `source docs/tapes/setup.sh && postui "$POSTUI_DEMO"`
Enter
Sleep 2s
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
Type "describe"
Sleep 500ms
Enter
Sleep 600ms
Type "names and companies"
Sleep 500ms
Show
Sleep 500ms
Screenshot docs/images/describe.png
