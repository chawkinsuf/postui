# Screenshot: a curl command pasted into the sidebar, at its name prompt.
# Run from the repo root: vhs docs/tapes/curl-import.tape (see README.md here).
# The paste must arrive as a terminal (bracketed) paste; README.md here says
# what to check if it doesn't.

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/curl-import.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 1400
Set Height 860
Set Padding 12
Set Theme "Catppuccin Mocha"

Hide
Type `source docs/tapes/setup.sh && postui "$POSTUI_DEMO"`
Enter
Sleep 2s
# Open the users/ folder so the import lands in it.
Down
Sleep 300ms
Down
Sleep 300ms
Enter
Sleep 500ms
Copy "curl 'https://jsonplaceholder.typicode.com/todos?userId=1' -H 'Accept: application/json'"
Paste
Sleep 1s
Show
Sleep 500ms
Screenshot docs/images/curl-import.png
