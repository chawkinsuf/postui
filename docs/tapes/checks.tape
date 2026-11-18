# Screenshot: users/list-users sent, with its Checks tab open on the request
# and on the response.
# Run from the repo root: vhs docs/tapes/checks.tape (see README.md here).
# It launches with the Vim profile, as vim.tape does, so the tabs switch
# with gT, which VHS types as plain letters.

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/checks.gif
Source docs/tapes/include/settings.tape

# common.tape's launch, with the Vim profile.
Hide
Type `source docs/tapes/setup.sh vim && postui "$POSTUI_DEMO" || exit 1`
Enter
Sleep 2s

# Open the users/ folder, then list-users, as vim.tape does, and send it.
Type "jj"
Enter
Sleep 500ms
Type "jj"
Enter
Sleep 500ms
Type ":send"
Enter
# The status line: "200", then the time ("158 ms", or "1.3 s" past a second).
Wait+Screen@15s /200 +\d/
Sleep 500ms

# Into the request's tabs, and back from Headers to Checks, the last tab.
Ctrl+W
Type "l"
Sleep 300ms
Type "gT"
Sleep 300ms
# The same in the response, once its Checks tab has its mark: the checks
# are done.
Ctrl+W
Type "j"
Sleep 300ms
Wait+Screen@15s /Checks [✓✗]/
Type "gT"
Sleep 800ms
Show
Sleep 500ms
Screenshot docs/images/checks.png
# A frame after the screenshot, so the tape doesn't end on it.
Sleep 500ms
