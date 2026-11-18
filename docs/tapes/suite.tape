# The README's suite GIF: postui suite -i at a shell, four requests at once,
# shown as they run.
# Run from the repo root: vhs docs/tapes/suite.tape (see README.md here).

Output docs/images/suite.gif
Source docs/tapes/include/settings.tape

# setup.sh's demo copy without the app: the shell moves into it, and
# create-post's secret gets a value, so the suite can send it.
Hide
Type `source docs/tapes/setup.sh && cd "$POSTUI_DEMO" && echo '[default]' >.postui/secrets.toml && echo 'api_token = "demo"' >>.postui/secrets.toml && clear || exit 1`
Enter
Sleep 1s
Show
Sleep 1s

# --env default: the environment the secret is under, whatever the picker
# would start on.
Type@90ms "postui suite -i --env default"
Sleep 500ms
Enter
Sleep 1.2s

# The picker: down to At once, and up from 1 to 4.
Down
Sleep 400ms
Down
Sleep 400ms
Right
Sleep 400ms
Right
Sleep 400ms
Right
Sleep 1s
Enter

# The total line comes last: OK or FAIL, then the count.
Wait+Screen@20s /(OK|FAIL) +\d+ requests/
Sleep 4s
