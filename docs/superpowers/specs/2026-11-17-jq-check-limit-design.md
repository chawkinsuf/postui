# A time limit for each jq check (#310)

Branch `jq-check-limit`, from `main` at `a207b70` (#312 merged).

Amended 2026-11-17 by #308 (section 7): the key is now `jq_limit_ms`,
`run_limited` is folded into `jq::run`, and every jq run stops at the
limit. Sections 1 to 6 keep #310's record under its names.

## 1. The problem and what was found

A jq check in a request's `[checks]` runs as long as its filter takes.
One that never ends (`last(range(1e12)) > 0`, a loop written by mistake)
hangs `postui run` and `postui suite` on that request, and in the app
leaves the response's Checks tab on `checking…` while its job keeps a
blocking-pool thread and its read of the body until postui quits; every
re-send adds another (checks tabs spec, section 7, detail 4). A check
that builds an endless array (`[repeat(1)] | length == 1`) also grows
memory until the process dies.

> "Why would we let checks run forever even in the cli? Each jq check
> should have a max runtime or it fails." (you, #309)

All three run a request's checks through one function,
`postui_core::checks::evaluate`, which runs each filter with `jq::run`.

**How a run can be stopped in-process.** Verified against jaq-core 3.1.1
and measured on a prototype (built, measured, then removed; nothing of it
is on the branch):

- jaq evaluates every term through `Id::run`
  (`jaq-core-3.1.1/src/filter.rs:464-466`), which starts with
  `cv.0.lut()`; `Ctx::lut` (`filter.rs:116-118`) calls the data's
  `HasLut::lut`, which is postui's (`crates/postui-core/src/jq/mod.rs:33-37`).
  So every step of a filter that loops through jq definitions (`repeat`,
  `until`, `while`, `recurse`, `limit`, `reduce`/`foreach` bodies, a
  recursive `def`) passes through postui's code, with or without output.
- jaq's native `range/3` (`jaq-core-3.1.1/src/funs.rs:31-36`, `55-71`) is
  a plain Rust iterator that never calls `lut()`, and `first`, `last`,
  `limit` and `[...]` pull it in Rust. So `last(range(1e18))` and
  `[range(1e10)]` loop without passing through postui. `range/1` and
  `range/2` are defined on top of it (`defs.jq:16-17`). No other native
  in jaq-core, jaq-std 3.0.3 or jaq-json 2.0.3 loops without input: the
  rest walk values the run already holds, and regex (`regex-bites`, the
  regex-lite engine) is linear in its input.
- Stopping: a counter in `lut()` reads the clock every 1024 calls, and
  past the deadline unwinds with a marker value through
  `std::panic::resume_unwind`, which skips the panic hook (std docs).
  That matters: `main.rs:183-198` prints a worker thread's panic to
  stderr, which would land on the TUI's screen. `catch_unwind` around the
  run catches the marker and turns it into the run's error; any other
  payload is re-raised. A filter's `try`/`catch`, `?` and `//` can't
  catch it, since it is not a jaq error. No profile sets
  `panic = "abort"` (`Cargo.toml`), which this needs.
- `range/3` is replaced by the same iterator with the same check on each
  step (about 20 lines, a copy of jaq's).

**Measured** (release build, a 4.9 MB body of 60,000 objects, best of 7):

| Filter | No limit | With the hook and a deadline |
|---|---|---|
| `.data.items \| map(select(.status == "active")) \| length > 0` | 65.4 ms | 65.5 ms |
| `[.data.items[] \| .price] \| add > 0` | 45.5 ms | 45.4 ms |
| `[.data.items[] \| .tags \| join(",")] \| unique \| length == 1` | 253.6 ms | 252.1 ms |
| `.data.items \| group_by(.status) \| length == 2` | 48.7 ms | 48.5 ms |
| `reduce range(3000000) as $x (0; . + $x) > 0` | 431.6 ms | 432.3 ms |
| `[limit(1000000; repeat(1))] \| length == 1000000` | 110.8 ms | 110.3 ms |
| `last(range(30000000)) > 0` | 460.1 ms | 466.4 ms |

The overhead is within run-to-run noise (the unmodified code varied by
up to 12 ms between two runs). With a 1 s limit, every one of these
stopped and returned the error: `last(range(1e18))` 1000.2 ms,
`def f: f; f` 1000.1 ms, `until(false; .)` 1000.1 ms,
`reduce range(1e18) as $x (0; . + 1)` 1000.1 ms, `last(repeat(1))`
1000.1 ms, `label $out | repeat(1) | select(false)` 1001.0 ms,
`try (last(range(1e18))) catch true` 1000.1 ms,
`(last(range(1e18)))? // true` 1000.1 ms, a nested loop over the body's
60,000 items 1005.9 ms, `[repeat(1)]` 1021.2 ms (peak memory 149 MB),
`[limit(1e12; repeat(1))]` 1018.3 ms, `[range(1e10)]` 1083.2 ms (peak
640 MB). The time past the limit is the run's values being freed.

**What it does not stop.** A native step already running finishes before
the next check of the clock: one huge allocation (`"x" * 1e10`) still
fails the process, and a deep recursion (`def f: 1 + f; f`) still
overflows the stack (#282). A sort or `group_by` of a large array the
run built is bounded by what it could build within the limit, so it ends,
late. An endless array grows for the limit's length, then is freed.

## 2. Rules

| # | Rule | Source |
|---|---|---|
| L1 | Each jq check has a maximum runtime; a check that passes it fails, in `run`, `suite` and the app alike. | you ("Each jq check should have a max runtime or it fails") |
| L2 | The limit is 1 second by default. | you ("Yes I already confirmed 1 second.") |
| L3 | The limit is per check: each jq check gets the whole limit, not a budget shared by the request's checks. | mine (approved) |
| L4 | The time counts from the check's run, after the body is read: reading the body into jq's document is not counted, and is done once for all of a request's checks. | you (1 second "counted after the body is read") |
| L5 | The limit is a project setting, `jq_check_limit_ms` in `project.toml`, in whole milliseconds; absent means 1000. It has no control in the app's Settings tab. (2026-11-17, #308: renamed `jq_limit_ms` before any release, since it now sets every jq run; section 7, B5.) | you ("Might as well make it configurable on a project level I think. Could be only in config for now."); that this means `project.toml`, with no Settings-tab control: mine (approved) (Q1: A, 2026-11-17) |
| L6 | The run is stopped in-process: postui's `lut()` hook checks a deadline, and `range` is replaced by one that checks it; past it the run unwinds, frees its thread and memory, and the check fails. | mine (approved) ("C and in process") |
| L7 | A stopped check fails with the line `<filter>: stopped at the <N> ms limit`, the same in `run`'s `check failed:` line, `run -v`, `suite`'s rows, both JSONs' `failure`, and the app's Checks tab; `<N>` is the limit in use. The key is named in the docs, not the line. | mine (approved) (Q2: A, 2026-11-17) |
| L8 | The request's other checks still run, each with its own limit; a request with a stopped check fails as one with any failed check does, and the suite goes on to the next request. | you (ticket: "the rest of the request's checks, and the rest of the suite, go on") |
| L9 | A bad `jq_check_limit_ms` (0, negative, a fraction, a string, above 4294967295) is a bad `project.toml`, as any bad value there is today: `run`, `suite` and `check` refuse with the file named; the app can't open the project; an outside edit while the app is open keeps the last good value with the "could not read project.toml" warning. | mine (existing rule, "parse errors never default") |
| L10 | Nothing else changes: the jq bar, saved views, completion and `run --jq` run without a limit (#308 has the bar). (2026-11-17, #308: replaced; they stop at the same limit, section 7, B1 to B3.) | mine (ticket scope) |

## 3. Design

**Core, `jq` module** (`crates/postui-core/src/jq/mod.rs`):

- `Globals` gains `stop: &Stop`. `Stop` holds an optional deadline and a
  call counter (`Cell<u32>`); `Stop::tick` counts and, every 1024 calls,
  compares the clock with the deadline, and past it calls
  `resume_unwind` with a private marker type. With no deadline it does
  nothing. `HasLut::lut` calls `tick` first. (Replaced in review round 1,
  see the amendment below: a flag a timer thread sets.)
- `funs()` drops jaq-core's native `range/3` and adds postui's, which
  ticks on each value it yields. Its outputs and errors are jaq's (a
  copy of `jaq-core/src/funs.rs:55-71`).
- `for_each_output` takes the `Stop` and runs its loop inside
  `catch_unwind`: the marker becomes `Err(JqError::Runtime { message:
  "stopped at the <N> ms limit" })`, the whole run's error, as the output
  cap's is; any other panic is re-raised.
- New `pub fn run_limited(code, doc, limit: Duration) -> Result<JqRun,
  JqError>`: `run` with a deadline at `now + limit`, set before the filter
  compiles. `run` and `run_with_cap` keep their signatures and pass no
  deadline, so the bar, completion and `run --jq` behave as before.
- The `Stop` is the reusable part: #308 can give it a cancel flag beside
  the deadline. Not built here.

**Core, `checks`**: `evaluate(checks, status, timing, limit: Duration,
doc)` runs each filter with `jq::run_limited(filter, doc, limit)`. The
stop arrives as the run's `Err`, so it becomes `Outcome::Jq { got: [],
error: Some("stopped at the 1000 ms limit") }` and its line is the
existing `"{filter}: {error}"`. `Outcome` and the JSON shapes don't
change.

**Core, project** (`project/meta.rs`): `ProjectMeta` gains
`jq_check_limit_ms: Option<NonZeroU32>` (serde refuses 0, negatives,
fractions and strings) and `ProjectMeta::jq_check_limit() -> Duration`,
1000 ms when absent. Nothing writes the key: `Project::init` doesn't add
it, and the TUI's `project.toml` edits keep it (they edit the document in
place).

**Callers** read the limit from the open project and pass it on:

- `run` (`cli/run.rs`, `record`): `s.project.meta().jq_check_limit()`.
- `suite` (`cli/suite/mod.rs`): read once before the sends start, moved
  into each send's `evaluate`.
- The app (`app.rs`, `Action::ResponseArrived`): read from
  `self.project` when the response arrives, so an outside edit to the
  key applies from the next response; passed into
  `ResponseData::check_run(checks, limit)` (`http.rs`, the
  `http::ChecksRun` that #311's tidy made, run by `http::run_checks`),
  and into `CheckState::on_arrival(data, checks, limit)`, which judges
  checks that read no body through `ResponseData::check_lines(checks,
  limit)`. With no project open, the default.

**What it ends from #312's section 7, detail 4:** a check job that holds
a never-ending check now ends within the limit for each of its checks, so
re-sends leave at most that long a job each, and a quit waits at most
that long for a running job.

**Contract changes:** `checks::evaluate` gains a parameter;
`ResponseData::check_run`, `check_lines` and `CheckState::on_arrival`
gain one; `jq::run_limited` is new; `ProjectMeta` gains a field (a
`project.toml` key) and `project::DEFAULT_JQ_CHECK_LIMIT` is new. No enum
changes.

Amended 2026-11-17 while building (mine): after merging `main`, the app's
check job is #311's `http::ChecksRun`, so the limit goes into
`check_run`/`check_lines` as planned and also into `on_arrival`, its one
other caller of `check_lines`. `run`'s `record` takes the limit as an
eighth argument.

Amended 2026-11-17, review round 1 (mine):

- **The stop is a flag a timer thread sets**, not the clock read every
  1024 steps. A run whose steps are each slow (`"x" * 1000000 |
  until(false; explode | implode)`) went 1024 steps between two reads of
  the clock: with a 50 ms limit it stopped after 8.6 s. Now
  `run_limited` starts a scoped timer thread that waits on a channel for
  the limit and sets `Stop`'s `AtomicBool` if the run hasn't ended;
  `tick` reads the flag on every step, so a run stops one step past the
  limit. The run's end (or unwind) drops the channel's sender, which
  ends the wait at once. Cost: a thread per jq check, about 0.1 ms
  (release, measured); the runs measured in section 1 stayed within
  noise (e.g. 61.2 vs 59.6 ms, 412.0 vs 405.5 ms), and the slow-step
  filter above, over 10 million characters with a 1 s limit, stopped at
  1.05 s.
- **The limit counts from the run, after the filter compiles**, not from
  before the compile.
- **`on_arrival` keeps its old signature**: it judges inline only checks
  with no jq check, so no limit comes into it.
- The app reads the limit as the response arrives, from the project as
  the app last read it (on a reload), not from the file at that moment.

Amended 2026-11-17, review round 2 (mine):

- **`on_arrival` makes the check run** with the project's limit, which
  its caller passes: `CheckState::on_arrival(data, checks, limit)` gives
  the state and, for checks that read the body, the run for the job;
  checks that read none it runs at once. `ResponseData::check_lines` is
  removed, so no caller judges with a stand-in limit.
- **`run`'s `record` takes the session** in place of its mask and the
  limit, and reads the limit from its project.
- A test pins the jaq crates' versions in `Cargo.lock` that `range`'s
  copy and section 1's claim about the other natives were checked
  against, and `range`'s test compares postui's copy with jaq's own.

**Docs:** the skill's `reference.md` lists `jq_check_limit_ms` under
`project.toml` and says in the `[checks]` paragraph that a jq check is
stopped at the limit; the checks tabs spec's section 7 detail 4 and A8
and the suite checks spec get a dated #310 paragraph.

## 4. Tests

| Rule | Test |
|---|---|
| L1, L6 | `jq`: `run_limited` with a 50 ms limit stops `last(range(1e18))`, `[range(1e10)]`, `[repeat(1)]`, `def f: f; f`, `until(false; .)`, and `try (last(range(1e18))) catch true`, each with the error `stopped at the 50 ms limit`, well under 5 s. |
| L1, L6 | `jq` (review round 1): `"x" * 1000000 \| until(false; explode \| implode)` with a 50 ms limit stops in under 1 s (it took 8.6 s with the clock read every 1024 steps). |
| L6 | `jq`: postui's `range` gives what jaq's did: `[range(5)]`, `[range(2; 8)]`, `[range(0; 10; 3)]`, `[range(5; 0; -2)]`, `[limit(3; range(0; 1; 0))]`, `[range(0.5; 3)]`, `range(0; 3; "a")` gives 0 then an error (amended while building: `range(1; "a")` never ends, since a number is below every string); over `run` and `run_limited` alike. |
| L6 | `jq`: a filter that ends gives the same `JqRun` from `run` and from `run_limited` (the existing fixtures). |
| L10 | `jq`: `run` has no limit (a filter of about 100 ms with a deadline of 1 ms in `run_limited` is stopped, through `run` it ends). |
| L2, L4, L5 | `meta`: absent key gives 1000 ms; `jq_check_limit_ms = 2500` gives 2.5 s; L9's bad values fail to parse. |
| L9 | `project`: opening a project whose `project.toml` holds `jq_check_limit_ms = 0` is refused naming `project.toml`. |
| §3 | app (`project_tests.rs`): a change on the Project tab keeps `jq_check_limit_ms` in the file. |
| L3, L7, L8 | `checks`: a request with a status check, a never-ending filter, a second never-ending filter and `.ok`: the status line, two `<filter>: stopped at the 50 ms limit` lines, and `.ok` passed, in order. |
| L4 | `checks`: the body's read is outside the limit (a `doc` closure that sleeps 100 ms, a 50 ms limit, a filter that ends: it passes). |
| L1, L7, L8 | `cli_run` (integration): a project with `jq_check_limit_ms = 50` and a request with `last(range(1e18)) > 0`: `run` exits 1 with `check failed: last(range(1e18)) > 0: stopped at the 50 ms limit`, and its JSON's `failure` says the same. |
| L8 | `suite`: two requests, the first with a never-ending check: the first fails with that line, the second passes, the run ends. |
| L1, L7 | app (`checks_tests.rs`): a response arriving for a request with a never-ending check and the project's limit at 50 ms gets its Checks tab's failed line and leaves `checking…`. |
| L1 | TUI in tmux: a request with `last(range(1e18)) > 0` sent: the tab says `checking…`, then about 1 s later the failed line. |

## 5. `mine` choices, for veto

Answered 2026-11-17 ("A for both", the user's pick of the recommended
options, so mine (approved)): Q1, the setting lives in `project.toml`
(shared with the team and with `run`/`suite`), not the user's own
`config.toml`; Q2, L7's line is `stopped at the 1000 ms limit` without
the key's name, which the docs give. Still open for veto:

- The key's name, `jq_check_limit_ms`, top level in `project.toml`, in
  milliseconds like `max_ttfb_ms`. Not a project-level `[checks]` table,
  which would read as checks for every request.
- No "no limit" value (the user: every check has a maximum); the largest
  is about 49 days.
- L9: a bad value fails `project.toml` like any other bad value, rather
  than falling back to 1 s with a warning.
- ~~The clock is read every 1024 term evaluations~~ (replaced in review
  round 1): a timer thread per jq check sets a flag each step reads; a
  stop comes one step past the limit, plus the time to free what the run
  built (up to 85 ms seen).
- A postui older than this branch refuses a `project.toml` that has the
  key (`ProjectMeta` denies unknown keys), as it would any newer key.
- L10: the jq bar, views, completion and `run --jq` get no limit here.

## 6. Plan

One sequence, TDD:

1. `meta`: the field, its accessor, parse tests.
2. `jq`: `Stop`, the `lut()` hook, `range/3`, `catch_unwind`,
   `run_limited`; its tests.
3. `checks::evaluate` with the limit; its tests; the callers in `run`,
   `suite`, `http`/app; their tests.
4. Docs: `reference.md`; dated paragraphs in the checks tabs and suite
   checks specs.
5. Suite, clippy, fmt; the TUI check in tmux.

## 7. #308: every jq run stops at the limit (2026-11-17)

Branch `jq-bar-limit`, from `main` at `e616555` (#310 merged, not
released).

**The problem.** A filter in the jq bar that never ends (`[limit(1;
repeat(1) | select(. == 2))]`) keeps a blocking-pool thread busy until
postui quits, and quitting waits for it: Tokio's `Runtime` drop waits on
every blocking task, so the process stays after the terminal is restored.
A saved view with such a filter starts one more run on every response of
its request. The bar moving on only forgets the run's number
(`Response::apply_jq`, `drop_pending_jq` clear `JqBar::pending`); the run
itself (`App::spawn_jq_run` → `jq_worker` → `jq::run`) had a `Stop`
nothing set. Completion's key lookup (`jq_complete_worker` →
`complete::keys_at`) and `postui run --jq` (`cli/run.rs`, `run_jq`) were
the same.

| # | Rule | Source |
|---|---|---|
| B1 | The jq bar's run (a typed filter, the scratch or a saved view's) stops at the project's limit. The bar shows `stopped at the <N> ms limit` as its error, with the last result kept behind it as for any filter that fails. | mine (approved) (#308 Q1: A) |
| B2 | Completion's key lookup stops at the limit too, and offers the keys it found before the stop. | mine (approved) (#308 Q1: A); the keys found before: mine |
| B3 | `postui run --jq` (and `--view`) stops at the limit: the stop is the filter's error, printed and in the JSON's `jq.error`, and the run exits 1 as for any failed filter. | mine (approved) (#308 Q1: A) |
| B4 | One limit per run, counted from after the filter compiles and the body is read, as for a check (L4). | mine |
| B5 | The key is `jq_limit_ms` (was `jq_check_limit_ms`), renamed with no alias since it was never released; `ProjectMeta::jq_limit()` and `project::DEFAULT_JQ_LIMIT` likewise. | mine (approved) (#308 Q2: A) |
| B6 | A bar run the worker parsed the body for, then stopped (or failed whole), still hands back the parse, so the next run doesn't read the body again. It shows as before: the error, the last result behind it. | mine |
| B7 | Quitting stops every jq run the app has going: the app sets one flag as it quits, and each bar run and completion lookup reads it beside its limit and stops, so quit no longer waits on jq whatever `jq_limit_ms` is. Runs typed past still run to the limit. (2026-11-17, added from the review round 2 question.) | mine (approved) (#308 round 2 question: A) |
| B8 | The response's check job reads the same flag, so a quit doesn't wait on its jq checks either. | mine |

There is no cancel: a run the bar has moved past still runs until it ends
or reaches the limit (option B on the ticket, not taken). ~~A quit waits at
most that long for a run still going.~~ (2026-11-17: replaced by B7 and
B8; option C, Tokio's `shutdown_timeout`, and option D, a separate bar
limit, were not taken.)

**Design.**

- `jq::run(code, doc, limit)` is the one way to run a filter: #310's
  `run_limited` is folded into it, and `run_with_cap(code, doc, cap,
  limit)` takes the limit too. No unlimited run is left.
- A stopped run fails with `JqError::Stopped { limit }`, whose message is
  `stopped at the <N> ms limit`. `complete::keys_at(expr, doc, limit)`
  runs with the limit and returns `Keys { keys, stopped_at }`: the keys it
  had when stopped, and the limit if it was.
- The app: `App::sync_jq` passes the open project's limit
  (`meta().jq_limit()`) to `apply_jq` and `refresh_jq_completion`, which
  use it inline and put it in `JqRunRequest` and `JqCompleteRequest`;
  `jq_worker` / `jq_complete_worker` run with it. What the bar remembers
  having applied (`jq_applied`) and the keys completion cached carry the
  limit their run or lookup was stopped at, if it was. A stopped one is
  remembered like any result, since running it again on every update would
  keep a core busy, the problem this section fixes; once the app reloads
  `project.toml` with another limit it runs again under that one. A filter
  or lookup that ended, the empty filter and one that doesn't compile are
  left alone under another limit. A run or lookup still going is awaited
  and judged when it lands.
  (Review round 1, 2026-11-17: the limit was first held on `Response` and
  keyed nothing, so a filter stopped at the old limit stayed stopped until
  edited or re-sent. Round 1 then keyed every memo on the limit; review
  round 2, 2026-11-17, found that re-ran finished filters and reset the
  view, its selection and sideways scroll included, for any filter or none,
  on a limit change. Only a stop's limit is kept now, and a run going at
  the old limit is no longer superseded, which would have left two runs
  busy, since nothing cancels the first.)
- `jq_worker` (B6): the parse the worker made comes back beside the
  result in `Action::JqRunFinished { doc, result }`, failed or not, and
  `attach_jq_result` keeps it; the result is the run's own `Result`.
  (Review round 2, 2026-11-17: a whole-run error was first handed back as
  an empty run carrying the error, to carry the parse.)
- `run --jq`: `run_jq` takes the session's project limit.
- Quit (B7, B8; 2026-11-17, after review round 2): `jq::run`,
  `run_with_cap`, `complete::keys_at` and `checks::evaluate` take `quit:
  Option<&AtomicBool>`, which `Stop` reads on each step beside its timer's
  flag. `postui run`, `suite` and `check` pass `None`, so nothing there
  changes. The app owns one `Arc<AtomicBool>`, set when the `App` is
  dropped, which is as `main.rs`'s loop returns, however it returns, and
  before the Tokio runtime's drop waits on the blocking pool.
  `jq_worker` and `jq_complete_worker` take it, and
  `ResponseData::check_run` and `CheckState::on_arrival` take a clone for
  the check job. A run the flag stops reports the limit's stop, as a run
  that timed out does; nothing reads it, since the app that would is gone.
  No handle per run (option B, not taken).

**Tests.**

| Rule | Test |
|---|---|
| B1 | `jq`: `run` with a 50 ms limit stops the never-ending filters of section 4 (the L1 tests, now over `run`). |
| B1 | response (`components/response.rs`): a never-ending filter applied inline with the bar's limit at 50 ms shows `stopped at the 50 ms limit` as the bar's error, the earlier result kept; over the pool path, the request carries the limit. |
| B1 | app (`tests.rs`): with `jq_limit_ms = 50` in `project.toml`, a never-ending filter typed in the bar ends with that error. |
| B2 | `complete`: `keys_at` over a never-ending expression with a 50 ms limit returns in well under 5 s, with the keys found before the stop and the limit it stopped at. |
| B3 | `cli_run` (integration): `jq_limit_ms = 50` and `--jq 'last(range(1e18))'`: exits 1 with `stopped at the 50 ms limit`. |
| B5 | `meta`, `project`, `project_tests.rs`, `checks_tests.rs`, `cli_run`: the existing tests under the new name. |
| B1, B2 | response: a filter, and a completion lookup, stopped at one limit runs again under another; one that ended, the empty filter and one that doesn't compile don't, and the view keeps its sideways scroll; one going is awaited; app: a raised `jq_limit_ms` read from `project.toml` runs the stopped filter again. |
| B6 | app: `jq_worker` with a body and a never-ending filter hands back the parsed document beside the error; response: a failed pool run's parse is kept, and the failure shows as any does. |
| quit | TUI in tmux: a never-ending filter in the bar shows the error about 1 s later, and quitting right after typing it ends the process within the limit. |
| B7 | `jq`: a run, and `keys_at`, over a never-ending filter with a 60 s limit stop soon after the quit flag is set from another thread. |
| B7 | app: under a Tokio runtime with `jq_limit_ms = 60000`, a never-ending filter typed in the bar, and a completion lookup that never ends, both end soon after the `App` is dropped. |
| B8 | `checks`: `evaluate` with a set quit flag stops a never-ending check; app: a check job's never-ending check ends soon after the `App` is dropped. |
| B7 | TUI in tmux: `jq_limit_ms = 60000`, a never-ending filter typed in the bar, quit at once: the process exits within about a second. |

**`mine` choices, for veto:** B2's keys found before the stop; B4; B6; B8;
the bar's memo and completion's cache carry a stop's limit (a run stopped
by a slow moment stays until the filter, response or limit changes); no
cancel of a run the bar moved past.
