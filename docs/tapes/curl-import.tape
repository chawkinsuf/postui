# Screenshot: a curl command imported in the sidebar, open as a request and
# sent.
# Run from the repo root: vhs docs/tapes/curl-import.tape (see README.md here).
# Copy puts the command on your system clipboard, replacing what's there.

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/curl-import.gif
Source docs/tapes/include/common.tape

# Open the users/ folder so the import lands in it.
Source docs/tapes/include/open-users.tape
# ctrl+v in the sidebar imports the curl on the clipboard. Not VHS's Paste,
# which types the text as keys.
Copy "curl 'https://jsonplaceholder.typicode.com/todos?userId=1' -H 'Accept: application/json'"
Ctrl+V
Sleep 1s
# Keep the suggested name; the import opens. Then send it.
Enter
Sleep 800ms
Ctrl+R
Wait+Screen@15s /200 +\d+ ms/
Sleep 800ms
Show
Sleep 500ms
Screenshot docs/images/curl-import.png
# A frame after the screenshot, so the tape doesn't end on it.
Sleep 500ms
