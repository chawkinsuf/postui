# Screenshot: the theme picker with its contrast control.
# Run from the repo root: vhs docs/tapes/themes.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/themes.gif

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
Alt+t
Sleep 800ms
Show
Sleep 500ms
Screenshot docs/images/themes.png
