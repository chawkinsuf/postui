# Suite checks beyond the status code — Design

Task #221. Status: **approved 2026-11-14.** The three points put to the user
(**Q1–Q3**, ticket comment 1348) were each answered A ("A for each of them",
comment 1349), so the recommended forms below are what gets built; they are
`mine (approved)` in the rules table. Everything marked `mine` drew no veto
and stays listed in §6 for a second look. The user also asked for a TUI
follow-up, filed as #302 "Show request checks in the TUI" (§2.5).

## 1. The problem

`postui suite` passes a request on any status below 400
(`SuiteResult::passed`, `FIRST_FAILING_STATUS` in `cli/run.rs`). In the vLLM
dogfood (2026-10-30) all 34 requests passed, and each answer still had to
be read by hand with `run --view`: does the JSON-schema chat parse, does the
regex output match, does the image answer name the colour. Each of those is
a one-line jq test on the body:

```
.choices[0].message.content | fromjson | has("name")
.choices[0].message.content | test("^[0-9]{3}-[0-9]{4}$")
.choices[0].message.content | ascii_downcase | contains("red")
```

The other gap is the status itself: a request that tests an error path
(expects 404) fails the suite today, whatever it returns.

Response checks were a non-goal of the CLI spec ("for now status and timing
i think", you, spec 2026-10-28 §1) and of the live view (spec 2026-11-13 §1).

### What exists

- **Request files** are read by `HttpRequest::from_toml_str` with
  `deny_unknown_fields`, and written whole by `HttpRequest::to_toml_string`.
  The TUI rebuilds the request from the editor's own fields
  (`Editor::current_request`) and replace-with-curl keeps a list of fields
  (`curl_flow.rs`), so any new key has to be carried through the editor or a
  TUI save drops it.
- **jq**: `postui_core::jq` parses a body once (`JqDocument::parse`: JSON,
  NDJSON, or an SSE stream's events) and runs a filter over each value
  (`jq::run`), giving every output and the first value's error.
  `jq::check` compiles a filter without running it. `run --jq/--view`
  already use this (`run_jq` in `cli/run.rs`).
- **jq views** (`jq`, `[[jq_view]]`, `jq_current`) are display filters the
  response pane applies; the TUI creates, renames and deletes them freely.
- **`suite`** gets the whole body from `send_raw` but keeps only status,
  time and size. A result is printed as a table row with its error on the
  lines under it (plain), one JSON object (`request`, `method`, `url`,
  `status`, `elapsed_ms`, `size`, `error`), or a live-view row (`✓` / `✗` /
  `!`, error in place of status for a request not sent).
- **`postui check`** does not look at jq filters today.

## 2. The proposal

### 2.1 In the request file (Q1: A)

```toml
name = "Chat, JSON schema"
method = "POST"
url = "{{vllm-url}}/v1/chat/completions"

[checks]
status = 200
jq = [
  '.choices[0].message.content | fromjson | has("name")',
  '.choices[0].finish_reason == "stop"',
]
```

- `status`: one code or a list (`[200, 201]`). Given, it **replaces** the
  `< 400` rule for this request; not given, the rule stays. (mine)
  `status = []` names no code, so it reads as no `status`, and a save
  writes nothing for it (review round 1, 2026-11-14, mine).
- `jq`: filters that must each give `true`. They are their own filters, not
  the request's jq views. (Q1)
- Both are optional; a request with no `[checks]` behaves exactly as today.
- Cost, named with Q1: postui 0.4.1 and older refuse a request file with
  `[checks]` (it shows as broken there).

### 2.2 When a request passes (mine)

A request passes when it was sent and:

1. its status is one of `status`, or below 400 when there is no `status`;
2. every `jq` check passes. A check passes when the body reads as JSON (a
   stream too, as `run --jq` reads it), the filter gives at least one
   output, **every output is exactly `true`**, and no value's run errors.

Stricter than `jq -e` (last output truthy) on purpose: under `-e`, a
filter that returns a string, like `.content`, passes; here only `true`
does, so a check that forgot its comparison fails rather than always
passing.

A filter that doesn't compile is not a read error: the TUI still opens the
request. `postui check` reports it as an error naming the file, and the
suite fails that request with the compile error. A `status` that isn't a
code from 100 to 599 is a read error, as a bad jq view name is. (mine)

### 2.3 How a failed check reports (mine)

Each failed check is one line, masked like everything printed:

```
check failed: status 404, expected 200
check failed: .choices[0].finish_reason == "stop" gave false
check failed: .usage.total_tokens > 0 gave no output
check failed: .choices[0].message.content gave "I can't see an image" (clipped to 60 characters)
check failed: .choices[0] | fromjson: cannot parse …
check failed: .a == 1: not JSON: expected value at line 1 column 1
```

Amended 2026-11-14 while building (mine): a body that doesn't read gives
the reader's own message, as `run --jq` prints it (`not JSON: …`, or the
nesting limit), rather than a fixed "the body isn't JSON", so the line says
why. A status check given several codes says `expected 200 or 201`.

Amended 2026-11-14, review round 1 (mine): an error is clipped to 60
characters as a value is, since jaq's runtime errors hold the value they
failed on. A filter that doesn't compile says so over a body that doesn't
read too (R5), rather than the body's `not JSON`.

Amended 2026-11-14, review round 2 (mine): a failure is one line, its
lines joined by a space, so a filter written over several lines in the
file keeps stderr's one `postui:` line a message and the suite's lines
under the row. The JSON gives each check's `failure`, the line's text,
instead of the filter's outputs and error, which could be as large as the
body.

- **Plain text:** the row as today (the real status shows), the failure
  lines under it, where an error goes now.
- **Live view:** a `✗` row, its status in its usual colour; the first
  failure in red in place of the URL while the row is in the band (clipped,
  like the URL); once printed for good, every failure line goes under it,
  as a `!` row's error does. With a status check, the status is coloured
  by its verdict instead: green when it passes (a 404 it expects), red
  when it doesn't (review round 1, 2026-11-14, mine).
- **JSON:** each result gains `passed` (bool) and `checks`, one object per
  check, in file order: `{"check": ".a == 1", "passed": false, "failure":
  ".a == 1 gave false"}`; the status check is `{"check": "status",
  "expected": [200], "got": 404, "passed": false, "failure": "status 404,
  expected 200"}`. `failure` is `null` for a check that passed. `checks`
  is `[]` for a request without any, or one never sent.
- **Totals and exit:** unchanged in shape. `failed` now counts check
  failures too, so the exit is 1 when any check fails.
- A failing check prints no body. The way to look is `run --view` or
  `run --jq`, as today.

### 2.4 `run` (Q2: A)

`run` evaluates the same checks, with the same rule: exit 1 and a
`postui: check failed: …` line on stderr per failure; the body still prints
as now. A `status` check replaces the `≥ 400` rule there too, so a request
that expects 404 exits 0 and prints no `postui: 404 Not Found` line. `-v`
lists the checks after the response (and after the jq results, when there
are any), one `passed`/`failed` row each; `--json` gets the same `checks`
array.
`--dry-run` sends nothing and checks nothing.

### 2.5 The TUI (Q3: A)

The TUI shows nothing of the checks and has no way to edit them; it keeps
them. The editor carries `checks` unchanged so a save, an undo step, a
duplicate and replace-with-curl all leave them as they were. Showing them
is #302 "Show request checks in the TUI", filed at the user's word: "Make a
ticket to add something to the tui in a later update." (you, 2026-11-14).

### 2.6 Docs (mine)

`skills/postui/reference.md` gets `[checks]` beside `jq`; `SKILL.md`'s
`suite` and `run` paragraphs and exit codes say what a check does; the
`suite` and `run` usage texts say checks fail a request.

### 2.7 Time limits (#304, amended 2026-11-15)

Task #304 "Add time SLO to checks". Four questions (ticket comment 1354)
were each answered A ("A for each", comment 1356), so each is `mine
(approved)` (T1–T4 below).

```toml
[checks]
status = 200
max_ttfb_ms = 500
max_elapsed_ms = 2000
```

- **What is measured (T1).** Both figures `send_raw` already takes, each
  limit optional and independent: `max_ttfb_ms` against the time to the
  response head (status and headers in; DNS, connect, TLS, upload and
  redirects included), `max_elapsed_ms` against the time until the body is
  fully read. "First byte" is the head, so for a streaming answer it is not
  time to first token. The suite's JSON gains `ttfb_ms` on each result
  (`null` for a request not sent), so the figure a check judged is in the
  report.
- **Keys and values (T2).** Integer milliseconds named after the report
  fields. A limit passes at or under it. A value that is not a positive
  whole number is a read error, as a bad `status` is (R6). Not given, no
  check. A limit judges the whole milliseconds the report prints (the
  time truncated), so a line never reads `total 500 ms, limit 500 ms` and
  fails; 500.9 ms passes a 500 ms limit (`mine`).
- **A breach is a failed check (T3).** Lines:
  `check failed: first byte 812 ms, limit 500 ms` and
  `check failed: total 2310 ms, limit 2000 ms`. JSON:
  `{"check": "max_ttfb_ms", "limit": 500, "got": 812, "passed": false,
  "failure": "first byte 812 ms, limit 500 ms"}` (and `max_elapsed_ms`),
  after the status check and before the jq ones. Suite plain, live view
  (`✗`, first failure in place of the URL) and totals as §2.3; `run` as
  §2.4 (a `postui: check failed: …` line, exit 1, `-v` and `--json` list
  it). In the live view the time cell is also red when a time check
  fails. The TUI shows nothing of them and keeps them on a save, as R10;
  showing them stays with #302.
- **What the number includes (T4).** Measured as sent today. The docs say:
  suite sends on one pooled client, so the first request to a host pays
  DNS/TCP/TLS and later ones reuse the connection; `--jobs N` runs requests
  side by side, which can slow each one; a time check never cuts a request
  short (`--timeout` does that).

## 3. Rules table

| # | Rule | From |
|---|---|---|
| R1 | Checks live in the request file, `[checks]` with `status` and `jq` | mine (approved), Q1: A |
| R2 | jq checks are their own filters, not the request's jq views | mine (approved), Q1: A |
| R3 | `status` replaces the `< 400` rule; without it the rule stays | mine |
| R4 | A jq check passes only when every output is `true`, there is at least one, and no error | mine |
| R5 | A filter that doesn't compile: `check` error, suite failure, not a read error | mine |
| R6 | A bad `status` value is a read error | mine |
| R7 | Failure lines and JSON `passed`/`checks` as §2.3 | mine |
| R8 | Live view: `✗`, first failure in place of the URL, all under it when final | mine |
| R9 | `run` always evaluates checks the same way | mine (approved), Q2: A |
| R10 | The TUI keeps checks through every save path, shows none | mine (approved), Q3: A |
| R11 | Showing checks in the TUI is a later task, #302 | you: "Make a ticket to add something to the tui in a later update." |
| T1 | Time limits on both first byte and total, each optional; suite JSON gains `ttfb_ms` (#304) | mine (approved), Q1: A, 2026-11-15 |
| T2 | `max_ttfb_ms` / `max_elapsed_ms`, integer ms; pass at or under; not a positive whole number is a read error | mine (approved), Q2: A, 2026-11-15 |
| T3 | A breach is a failed check: lines, live `✗` and red time cell, JSON `{check, limit, got, passed, failure}` after status and before jq; `run` stderr line and exit 1 | mine (approved), Q3: A, 2026-11-15 |
| T4 | Measured as sent today; docs name connection reuse, `--jobs`, and that a time check never aborts a request | mine (approved), Q4: A, 2026-11-15 |

## 4. What changes

- `postui-core/src/model.rs`: `HttpRequest` gains `checks: Checks`
  (`status: Vec<u16>`, `jq: Vec<String>`), read and written by
  `from_toml_str` / `to_toml_string` (written after the jq views, before
  `[params]`; nothing when empty).
- `postui-core`: one function that runs a request's checks over a status
  and body and returns each check's outcome, so `run` and `suite` share it.
- `cli/suite`: keep the body until the checks have run; `SuiteResult` gains
  the outcomes; `passed()` uses them; plain, JSON and live paint as §2.3.
  jq runs off the async loop so a large body doesn't stall the live view's
  ticks.
- `cli/run.rs`: the same, per §2.4.
- `cli/checks.rs`: the pass rule (R3 with every check), one function `run`
  and `suite` both derive pass/fail from, rather than storing it (review
  round 1).
- `cli/check.rs`: compile each check filter.
- `components/editor.rs`, `app/curl_flow.rs`: carry `checks`.
- Docs per §2.6.

## 5. Tests

- Model: `[checks]` round-trips; empty writes nothing; a bad `status` is a
  read error; old files read unchanged.
- The check runner: `true` passes; `false`, no output, a string, an error,
  a non-JSON body each fail with the §2.3 wording; a stream body; a status
  list.
- Suite: a 404 with `status = 404` passes; a 200 with a failing jq check
  fails; text, JSON and the live row as §2.3; the exit code.
- `run`: exit code and stderr lines.
- `check`: a filter that doesn't compile is an error naming the file.
- TUI: a request with checks, edited and saved, keeps them; replace with
  curl keeps them.

## 6. `mine` choices, for veto

R3–R8 above; the failure wording in §2.3; values and errors clipped to 60
characters; `status = []` read as no status check; the status coloured by
its check; a failure on one line; the JSON's `failure` in place of a
filter's outputs;
the TOML placement in §4; the docs in §2.6. Also left out, each could be
its own task: header checks, response-time limits (since taken up by #304,
§2.7), checks on a body that isn't JSON (as a string), and naming a check.
