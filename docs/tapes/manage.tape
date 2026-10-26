# Screenshot: the Manage screen with a variable's value in each environment.
# Run from the repo root: vhs docs/tapes/manage.tape (see README.md here).

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/manage.gif

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
Alt+r
Sleep 700ms
Down
Sleep 700ms
Show
Sleep 500ms
Screenshot docs/images/manage.png
