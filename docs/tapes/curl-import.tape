# Screenshot: a curl command imported in the sidebar, at its name prompt.
# Run from the repo root: vhs docs/tapes/curl-import.tape (see README.md here).
# Copy puts the command on your system clipboard, replacing what's there.

# VHS records a GIF whatever the tape is for; this one is thrown away.
Output docs/tapes/.out/curl-import.gif
Source docs/tapes/include/common.tape

# Open the users/ folder so the import lands in it.
Down
Sleep 300ms
Down
Sleep 300ms
Enter
Sleep 500ms
# ctrl+v in the sidebar imports the curl on the clipboard. Not VHS's Paste,
# which types the text as keys.
Copy "curl 'https://jsonplaceholder.typicode.com/todos?userId=1' -H 'Accept: application/json'"
Ctrl+V
Sleep 1s
Show
Sleep 500ms
Screenshot docs/images/curl-import.png
