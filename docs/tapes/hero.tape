# The README's hero: open a request, send it, and filter the response with jq.
# Run from the repo root: vhs docs/tapes/hero.tape (see README.md here).

Output docs/images/hero.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 1400
Set Height 860
Set Padding 12
Set Theme "Catppuccin Mocha"
Set TypingSpeed 90ms

Hide
Type `source docs/tapes/setup.sh && postui "$POSTUI_DEMO"`
Enter
Sleep 2s
Show
Sleep 1s

# Open the users/ folder, then list-users.
Down
Sleep 400ms
Down
Sleep 400ms
Enter
Sleep 700ms
Down
Sleep 400ms
Down
Sleep 400ms
Enter
Sleep 1.2s

# Send it.
Ctrl+R
Sleep 3s

# Filter the response.
Alt+q
Sleep 700ms
Type "map({name, email})"
Sleep 1s
Enter
Sleep 3s
