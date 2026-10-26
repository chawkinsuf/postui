#!/bin/sh
# The tapes' stand-in for `ai_cmd`: reads the "Describe a filter…" prompt
# like the real command would, and answers with a fixed jq filter, so a
# recording needs no AI account and comes out the same every time.
cat >/dev/null
echo 'map({name, company: .company.name})'
