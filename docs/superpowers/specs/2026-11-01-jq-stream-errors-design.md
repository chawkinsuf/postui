# jq: an error ends only its value's run (#229)

## Problem

On a streamed body (NDJSON), a filter runs once per value, but the first
runtime error ends the whole run and throws the other values' outputs
away. Lines where one lacks `items` make `.items[] | .id` show nothing but
`cannot iterate over null`, in the jq bar and in `postui run --jq` alike.
jq ends only that value's run: the other values' outputs still print, and
the error is reported with them (exit 5).

Where it happens: `for_each_output` in `crates/postui-core/src/jq/mod.rs`
already lets an error end only its value (completion uses that), but
`run_with_cap` turns the first error into the run's `Err`, so the outputs
collected so far are dropped there. The information is lost at that `?`.
Everything downstream (`JqRunOutput`, `land_jq_result`, `run_jq` in
`cli/run.rs`) only has "outputs or error" to work with.

The jq filter spec (2026-09-03, "`run` collects all outputs. An error
mid-stream discards the partial outputs") was written before streams, and
also says so for one value; jq keeps the outputs printed before the error
there too (`[{"a":1},2] | .[] | .a` prints `1`, then the error).

## Rules

| # | Behaviour | Source |
|---|---|---|
| R1 | A runtime error ends only the run of the value it happened on; the run goes on with the next value. | you (#218: "it would be good if it processed like jq did if you passed it ndjson"; this bug filed on your "anyone could make a filter that fails ... file a new bug ticket") |
| R2 | The outputs a value produced before its error are kept, as jq prints them. On a single-value body this means outputs before the error are kept too (changes the 2026-09-03 spec row). | mine (approved) |
| R3 | A run reports one error: the first. When more than one value failed, its message starts `first of N errors: `. N counts the runs that failed (a filter reading `input`/`inputs` takes several values in one run). *Amended 2026-11-01, review round 1:* the count first wrote `(N values failed)` at the end, but jaq's message holds the failing value, so on a long line the count ran off the bar's row. Leading with `N values failed: ` "makes it seem like all the errors were the same error" (you), hence "first of". | mine (approved); "first of" from your note |
| R4 | Compile errors, a body that is not JSON, and output past the cap stay whole-run errors with no outputs, as today. | mine (approved) |
| R5 | Bar: a run with an error and outputs worth showing shows those outputs in the tree, as a good run does (chip and filter badge not dimmed), with the error in red on the row under the bar. | mine (approved) |
| R6 | Bar: a run with an error and nothing worth showing (no output, all `null`, or output that is not JSON) is a failed run as today: the error under the bar, the previous tree kept, chip dimmed. | mine (approved) |
| R7 | `run --jq`: the outputs print (stdout, `jq.output`) and so does the error (the `postui: jq:` stderr line with its `-o` hint, `jq.error`); exit 1. `jq.output` is `null` only when the filter failed with no output (a filter that succeeds with none is `[]`, as before). | mine (approved) |
| R8 | The skill's "A filter that fails prints its error and no body" says the other values' results still print. | mine (approved) |

## Design

- **Core (`jq/mod.rs`).** `run`/`run_with_cap` return
  `Result<JqRun, JqError>`, where
  `pub struct JqRun { pub outputs: Vec<String>, pub error: Option<JqError> }`.
  `Err` is R4's whole-run errors only. The collecting closure stops
  propagating a filter error with `?`: it records the first error, counts
  failed values, and lets `for_each_output` go on to the next value (it
  already ends that value's run on an error). The count prefix (R3) is put
  on the message in core, so the bar and the CLI show the same text.
  `for_each_output`'s doc paragraph about `run` not doing this goes.
- **TUI.** `JqRunOutput` gains `error: Option<JqError>`, filled from
  `JqRun` by `jq_worker` and the inline path in `apply_jq`.
  `land_jq_result` shows the tree when there are outputs worth showing and
  sets `jq.error` to the run's error with `stale = false` (R5); otherwise
  an error takes the existing failed path (R6). `JqBar::stale`'s doc
  changes from "failed" to "nothing to show", since an error no longer
  implies a stale tree. The draw code needs no change: the error row
  already draws whenever `error` is set, and the chip/badge follow
  `stale`.
- **CLI.** `run_jq` returns the masked outputs and the error together;
  `record` fills both `JqReport` fields. `streams`/`text` already print
  outputs and then the error; `status` already exits 1 on an error.
- Removed: nothing beyond the "first error ends the run" rule.

## Tests

- core: a stream with one failing value yields the other values'
  outputs and the error (R1); a value's outputs before its error are kept
  (R2); two failing values give the first message after `first of 2 errors: `
  (R3); the cap and compile errors are still `Err` (R4). Rename
  `a_runtime_error_on_any_value_is_the_whole_result`.
- response: a landed run with outputs and an error shows the tree, not
  stale, error set (R5); with only nulls or no output and an error, the
  old tree stays and the chip is stale (R6).
- cli: `--jq` over an NDJSON body with one failing line prints the other
  results on stdout, the error on stderr, exits 1; the JSON has both
  `output` and `error` (R7).

## mine choices (approved 2026-11-01, reviewed with the user in chat)

- R2: keep outputs before the error on one value too (jq does). The
  alternative, dropping a value's partial outputs, needs a special case
  and isn't jq.
- R3: one error with a count, not every error. jq prints one line per
  failing value; a 1,000-line stream failing on every line would print
  1,000 identical lines to an agent's context, and the bar has one row.
  Cost: a second, different error message is not shown.
- R5: a run that partly failed looks like a good run apart from the red
  error row (chip and badge not dimmed), because the tree on screen is this
  filter's output.
- R7: exit 1 even though some results printed, so a script still sees the
  filter failed.
