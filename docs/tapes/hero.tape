# The README's hero: open a request, send it, and filter the response with jq.
# Run from the repo root: vhs docs/tapes/hero.tape (see README.md here).

Output docs/images/hero.gif
Source docs/tapes/include/common.tape

Show
Sleep 1s

# Open the users/ folder, then list-users.
Source docs/tapes/include/open-users.tape
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
Type@90ms "map({name, email})"
Sleep 1s
Enter
Sleep 3s
