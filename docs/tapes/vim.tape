# The README's Vim GIF: maximize the response with ctrl+w _, fold it, search
# it, and yank a Visual selection. The hero GIF already shows a send, so the
# take starts on the response.
# Run from the repo root: vhs docs/tapes/vim.tape (see README.md here).

Output docs/images/vim.gif
Source docs/tapes/include/settings.tape

# common.tape's launch, with the Vim profile.
Hide
Type `source docs/tapes/setup.sh vim && postui "$POSTUI_DEMO" || exit 1`
Enter
Sleep 2s

# Out of sight: open the users/ folder, then list-users, as open-users.tape
# does with arrows, and send it. `:send` opens the command palette, whose
# list redraws at every letter, so it stays out of the take.
Type "jj"
Enter
Sleep 500ms
Type "jj"
Enter
Sleep 500ms
Type ":send"
Enter
Wait+Screen@15s /200 +\d+ ms/
Sleep 500ms

Show
Sleep 1s

# Into the response window, and give it the whole column.
Ctrl+W
Type "l"
Sleep 300ms
Ctrl+W
Type "j"
Sleep 300ms
Ctrl+W
Type "_"
Sleep 1.2s

# Close every fold, then open one level: a line per user.
Type "zM"
Sleep 1s
Type "zo"
Sleep 1.5s

# Search opens the fold it lands in; n steps to the next match.
Type@120ms "/email"
Sleep 400ms
Enter
Sleep 1s
Type "n"
Sleep 1.2s

# Select up two lines and yank them.
Type "V"
Sleep 400ms
Type@400ms "kk"
Sleep 800ms
Type "y"
Sleep 3s
