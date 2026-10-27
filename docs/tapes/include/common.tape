# Shared by every arrows-profile tape (VHS `Source`): the recording settings,
# then postui launched out of sight on a fresh copy of the demo project
# (setup.sh). It ends hidden, so each tape does its own setup steps before
# `Show`.

Source docs/tapes/include/settings.tape

Hide
# A failed setup exits the shell, so the take visibly fails instead of
# typing its keys into bash.
Type `source docs/tapes/setup.sh && postui "$POSTUI_DEMO" || exit 1`
Enter
Sleep 2s
