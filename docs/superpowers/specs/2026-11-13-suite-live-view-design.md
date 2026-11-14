# `postui suite` live view — Design

Task #301. Written 2026-11-13 from a brainstorm with the user. Labels on
decisions: **you** is the user's own words, **mine (approved)** an option
I offered that the user picked, **mine** a call of mine they saw and did
not object to.

## 1. Intent

`postui suite` today (`crates/postui/src/cli/suite.rs`) sends every
request of a project, or of one space, one at a time in sidebar order,
gathers every result, and prints one plain table at the end. It was built
for agents (spec `2026-10-28-cli-for-agents-design.md`, §4 piece 2).

The user wants it to be good to run by hand at a terminal (you):

- "look much better, colors, formatting, maybe icons if they make sense";
- "show results as they come … with a waiting animation";
- "an option for a number of threads to use simultaneously, with each one
  updating as it goes";
- "an interactive way to choose the env, space, selector values";
- "a simple output like we have now for the agents".

### Goals

- At a terminal, a colored live view: each request's row appears with a
  spinner while it is in flight and turns into its result when it lands.
- `--jobs N` sends up to N requests at once. **Optional; by default it
  still goes one at a time** (you).
- `-i` opens a picker for environment, space, selector options and jobs
  before the run.
- Agents (no terminal) get today's plain output, unchanged.

### Non-goals

- Response checks or assertions; the suite still reports status, timing
  and size only (spec 2026-10-28, unchanged).
- Saving anything the picker chose: **"don't save the picks"** (you). The
  CLI never writes into the project, and still doesn't.
- A full-screen view, scrolling inside the view, or opening a response
  from the suite.
- Mouse support in the picker or the live view (mine: the live view is not
  in raw mode, and the picker is short enough for keys).
- Changing `run` or any other command's output.

### Anti-goals

- Changing the plain text or JSON an agent reads in any way it would
  notice, beyond the two additions in §6.
- Escape codes in a pipe, a file, or `-o`'s report.
- A suite that, run with no new flags, behaves differently than today
  apart from how it looks at a terminal. Order and one-at-a-time stay the
  default.

## 2. What exists today

- `suite::suite` builds a `Session` (`run::session`: opens the project
  read-only, applies `--env`/`--select`/`--var`, builds the mask, the
  prepare context and the clients), lists the slugs with
  `read::spaces_of` + `read::displayed` (sidebar order, folders after
  requests within a level), then for each slug calls `run::prepare_one`
  (needs `&mut Session`) and `http::send_raw`, building a `SuiteResult`.
  A prepare failure or send error is that row's result; the suite goes
  on.
- `SuiteReport::text` renders the table with `out::table_under` (cells
  aligned across all rows, an error on the lines under its row).
  `mod.rs`'s `Command::Suite` arm renders, writes `-o` if given, and
  prints with `cli_out::print_out`.
- `totals.elapsed_ms` is the sum of the results' own times.
- `RequestListing::display_name()` gives a request's on-screen name;
  spaces and folders have display names too, as the sidebar shows them.
- Dependencies already present: `crossterm` 0.29, `ratatui` 0.30, `tokio`
  (no `signal` feature yet). No CLI command uses color or detects a
  terminal today.
- `suite -i` is a usage error today ("unknown option '-i'"); on `run`,
  `-i` means `--include`.

## 3. Approaches

How the live view and picker get drawn. Under all three, preparing stays
sequential (it needs the session) and only sends run concurrently.

1. **Draw it ourselves with crossterm** — **chosen** (mine (approved)).
   The live band is redrawn by moving the cursor up its height, rewriting
   its rows and clearing below; a final row is printed once and scrolls
   like any line. The picker is the same in-place redraw with raw mode on.
   No new crates; does exactly what the mockups show; the rows are plain
   strings with SGR codes, so tests can assert on them without a terminal.
2. **ratatui's inline viewport** (`Viewport::Inline` + `insert_before`).
   Close, but the inline viewport has a fixed height and our band grows
   and shrinks from one row to a screen; we'd keep a screen-high blank
   area or rebuild the viewport on every change.
3. **indicatif + inquire/dialoguer.** Ready-made spinners and prompts, but
   their multi-progress is finish-ordered and their prompts are
   one-question-at-a-time; two new crates bent to fit the chosen design.

## 4. Which output a run gets (mine)

| Case | Output |
|---|---|
| `--json` | JSON at the end, as today (+ §6 additions). Never the live view, even at a terminal. |
| stdout is a terminal, no `--json`, no `--plain`, `TERM` not `dumb` | **The live view** (§5). |
| stdout is not a terminal (agents, pipes, `> file`), or `--plain`, or `TERM=dumb` | Today's plain text, printed once at the end, byte-for-byte as today for the same results. |
| `NO_COLOR` set (non-empty) | The live view without SGR color/bold/background codes; layout, icons and spinner unchanged. |
| `-o FILE` | FILE gets the plain text report (or JSON with `--json`), never the live view's codes. At a terminal, the live view still shows on stdout and its finish ends with the `written to <file>` line. |
| `-i` | Needs stdin and stdout to be terminals; otherwise a usage error: `-i needs a terminal to pick in`. `-i` with `--json` is allowed: pick, then JSON at the end. |

The terminal test is `std::io::IsTerminal` on stdout (and stdin too, for
`-i`).

**`--plain`, and `TERM=dumb`** (mine (approved), option A, from the
builder check): some agent harnesses and CI runners run commands under a
pseudo-terminal, where the live view's cursor movement would garble the
output (`NO_COLOR` drops only the color). `--plain` forces the plain path
at a terminal; `TERM=dumb` counts as no terminal, the usual convention.
`--plain` with `-i` is allowed (pick, then plain output); `--plain` with
`--json` is allowed and changes nothing (JSON is never live).

## 5. The live view

Mockups: `.superpowers/brainstorm/9497-1794616741/content/live-run-v2.html`
(local; the sketches below are the record).

### 5.1 Layout — grouped by space, request names (mine (approved), option B)

```
postui suite  env dev  space all  · 9 requests · 3 at a time

main
  users/
    ✓ GET    Get user        200  120 ms   512 B  https://dev/users/42
    ✗ DELETE Remove user     404   35 ms     9 B  https://dev/users/7
    ⠹ PUT    Update user          2.1 s           https://dev/users/42
  ✓ POST   List            201   80 ms   1.2 KB https://dev/list
  ! POST   Search          {{query}} has no value
  ⠼ GET    Stats                0.4 s           https://dev/stats
admin
  ⠧ GET    Health               0.1 s           https://dev/health
  · PATCH  Flags
  · GET    Audit log

 5 of 9 · 2 passed · 2 failed · 2.4 s
```

- **Header line:** `postui suite`, then `env <name>`, `space <name|all>`,
  any selector whose option differs from the saved one (`customer globex`)
  (mine), then `· N requests · one at a time` or `· N at a time`.
- **Space lines** in bold accent; **folder lines** (`users/`) indented
  under their parent, as the sidebar nests them. Names are the sidebar's
  display names. With `--space S`, the one space line still shows. (mine)
- **A request row:** icon, method (padded to 6), the request's display
  name, then the result: status, time, size, and the URL dimmed at the end
  (the URL was missing from the mockup's B; restored, mine). Names, statuses,
  times and sizes are aligned across rows that are on screen together
  (column widths fixed from the whole list where knowable: names and
  methods are known up front; time and size use fixed-width forms like
  `120 ms`, `2.3 s`, `512 B`, `1.2 KB`).
- **A row that never went out** (prepare refused, broken file, send error)
  shows `!` and the message in place of status/time/size; a broken file
  has no method or URL, as today.
- **Icons** (plain Unicode, no special font; mine): `✓` passed (status
  < 400), `✗` failed status (≥ 400), `!` not sent / send error, `·`
  waiting, braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` in flight (about 10 frames a
  second), `–` skipped (cancelled, §5.4). An in-flight row shows its
  running time in place of the status.
- **Colors** (mine): the terminal's 16 ANSI colors, so the output follows
  the user's terminal theme. Green 2xx, yellow 3xx, red 4xx/5xx, magenta
  for `!` rows, blue spinner, dim for waiting rows, times, sizes and URLs.
  Methods take the TUI's per-method hues mapped to the nearest of the 16.
- **Progress line** under the list while running: `N of M · P passed ·
  F failed · <wall time>`.

### 5.2 Order and the live band (mine (approved), option 1 + mine)

Rows are in **sidebar order however the run goes** (option 1: "fixed in
sidebar order"). Requests are started in sidebar order, so every row
above the first unfinished one is final. To keep this working when the
suite is taller than the screen (mine):

- Rows (and space/folder lines) above the **first unfinished request**
  are printed once, for good, and scroll into scrollback like any output.
- The **band** is everything from the first unfinished request down to
  the last row of the list, plus the progress line. It is redrawn in
  place on each tick and each result.
- Waiting rows below the newest started request are part of the band only
  while they fit; the band never draws more lines than the terminal is
  high minus one. Waiting rows that don't fit are left out, and a dim
  `… N more` line stands for them.
- **Throttle:** if the band (first unfinished row through the newest
  started row, plus the progress line) would exceed the terminal's height,
  no new request starts until it shrinks. A slow request holds the rows
  after it inside the band, finished; when it lands they print for good.
  The wait is not strictly bounded (`--timeout 0` is no limit, and a body
  that keeps trickling isn't cut off), but that is accepted (mine, from the
  builder check): the run can't finish before that request does in any
  mode, so the throttle only delays the rows after it; it never makes a
  run that would end, not end.
- Every band row is clipped to the terminal's width (a long URL ends with
  `…`, then the name if still too wide), so one row is one line and the
  redraw can count lines. A final row printed for good is not clipped: a
  long error wraps onto lines under its row, indented, as today's plain
  output does.
- A terminal resize is picked up on the next redraw (width and height are
  read each tick).
- With `--jobs 1` the band is the one in-flight row and the progress line.

### 5.3 The finish (mine (approved), option T)

When the last request lands, the band is printed for good, the progress
line is replaced by the totals line, and nothing is redrawn after:

```
 FAIL   9 requests · 7 passed · 2 failed · 3.0 s
```

- A badge on a colored background: ` OK ` green when nothing failed,
  ` FAIL ` red otherwise, ` CANCELLED ` (§5.4).
- **Just the totals line**, no recap of the failures: they stay where they
  happened in the list (you picked T over a recap).
- Then, as the plain output does: `written to <file>` with `-o`, and the
  warnings (open/prepare warnings, masked). Warnings are printed at the
  finish, not during the run, so the band never has to make room for them.
- The total time is **wall-clock** time of the sends (mine): with several
  at once the sum would be longer than the run took.

### 5.4 Ctrl+C mid-run (mine)

No new request starts and in-flight sends are dropped. Rows that didn't
finish turn into `– skipped` (dim). The band is printed for good, then
` CANCELLED ` with the totals counting only what finished (`skipped` is
added to the line: `9 requests · 4 passed · 1 failed · 4 skipped`). Exit
status 130. This needs tokio's `signal` feature (a feature of a crate we
already have, not a new crate). A second Ctrl+C is not special-cased: the
first already ends the run promptly. The plain and JSON outputs keep
today's behaviour on Ctrl+C (the process ends; nothing printed).

**`-o` on a cancelled live run is not written** (mine, from the builder
check): the plain path writes nothing on Ctrl+C either, and the plain and
JSON reports have no skipped state, so adding one would change the format
agents read. "Skipped" lives only in the live view's model. The finish
says so in place of `written to`: `not written to <file>: cancelled`. A
file left there by an earlier run is left as it was.

### 5.5 Cursor and stray input

The cursor is hidden while the band is live and shown again at the finish,
on Ctrl+C, and on any error path (a guard that restores it on drop, so a
panic doesn't leave it hidden). The live view does not enter raw mode;
keys typed during the run echo as in any command. (mine)

## 6. `--jobs`, and what plain and JSON gain

- **`-j, --jobs N`**: how many requests are in flight at once; default 1
  (you). N ≥ 1; `0` or a non-number is a usage error. N larger than the
  number of requests is fine.
- Preparing stays sequential in sidebar order (it needs `&mut Session`);
  sends run concurrently, at most N, each tagged with its row index. Rows
  are reported in sidebar order in every output, whatever order they
  finished in.
- With N > 1, requests that depend on an earlier one's side effects
  (create then delete) can race; the docs and the skill say so, and that
  `--jobs` is opt-in for that reason.
- **Plain text:** unchanged in layout. When N > 1 the heading line gains
  `  jobs N` after the space (so a reader of the output knows it ran
  concurrently); with N = 1 it is exactly today's line. The totals line's
  `ms` is the wall-clock total (below).
- **JSON:** gains `"jobs": N` at the top level; `totals.elapsed_ms`
  becomes the wall-clock time of the run's sends (mine). With N = 1 that
  is close to today's sum (it adds the small gaps between sends).
- The CLI-for-agents spec (§4 piece 2, decision 7 "Suite runs one at a
  time") gets an amendment pointing here: one at a time stays the default;
  `--jobs` lifts it on request.

## 7. The picker: `-i` (mine (approved), option A; "don't save the picks", you)

```
postui suite  my-api

  Environment   ‹ dev        ›
▸ Space         ‹ all        ›
  customer      ‹ acme       ›
  locale        ‹ en-US      ›
  At once       ‹ 1          ›

  9 requests

  ↑↓ move  ←→ change  enter run  esc cancel
```

- Opens **in place** (no alternate screen), in raw mode, drawn the same
  way as the band. When Enter is pressed, the form's lines are cleared and
  the run starts where the form was, its header naming what was picked
  (§5.1). Mockup: `.superpowers/brainstorm/9497-1794616741/content/picker.html`.
- **Rows:** Environment (the project's envs, by display name), Space
  (`all`, then each space), one row per selector the chosen environment
  has (its options, by name), and **At once** (1–16, then what `--jobs`
  gave if larger) (mine: the extra row). A project with no environments
  has no Environment row; one with no selectors has no selector rows.
- **Keys:** ↑/↓ (and `k`/`j`, mine: shared single letters) move between
  rows; ←/→ (and `h`/`l`) step the value, wrapping at the ends; Enter
  runs; Esc, `q` or Ctrl+C cancels: the form is cleared, nothing is sent,
  exit status 130 for Ctrl+C and 1 for Esc/`q`, no output. (mine)
- **Starting values:** the saved environment and selections, overridden
  by any `--env`, `--select`, `--space` and `--jobs` on the command line.
- **Changing the environment** rebuilds the selector rows from that
  environment's saved selections (a `--select` given on the command line
  applies only if its selector and option exist in the new environment).
- **Shared selectors** (`shared = true`, options in `variables.toml`) get
  rows too, after the environment's own, and their pick is kept across an
  environment change, since it is one pick for every environment (mine,
  from the builder check).
- **A selector with no selection** (cleared, so requests using its fields
  are refused today) starts at `(none)`, and `(none)` stays one of its
  values to cycle back to. A selector that has a selection doesn't offer
  `(none)`. So `-i` with nothing changed runs exactly what the run without
  `-i` would. (mine, from the builder check)
- The `N requests` line under the rows updates with the Space row.
- **Nothing is written**: picks live for this run only; the project is
  opened read-only as today.
- `--var` values aren't in the picker; they pass through as given.
- After the picker, the run is exactly as if the picks had been given as
  `--env/--space/--select/--jobs`: same session, same checks
  (`check_vars` included), same output rules (§4).

## 8. Layers that change

- **`cli/suite.rs`**: the run loop becomes a producer of row events
  (started, finished with a `SuiteResult`, skipped) in sidebar order,
  feeding either the plain collector (today's report) or the live view.
  `SuiteReport` gains `jobs`; totals use wall-clock time.
- **New `cli/suite/live.rs`** (or `cli/live.rs`; builder's call): the
  row/band model and the crossterm renderer; the rows are built as strings
  with SGR codes by functions that take the width, so they can be tested
  as text.
- **New `cli/suite/picker.rs`**: the form model (rows, values, key
  handling) apart from its drawing, so key handling is testable without a
  terminal.
- **`cli/mod.rs`**: parse `-j/--jobs`, `-i/--interactive` and `--plain`
  for `suite` only (today's `scan()` turns `-i` into `--include` for every
  command, so suite needs its own handling of it); usage text; the `Command::Suite` arm picks the output mode (§4).
  `run`'s `-i` (`--include`) is untouched.
- **`Cargo.toml`**: tokio gains the `signal` feature.
- No `postui-core` changes are expected; the run uses the session's
  existing project, listing and display names. Selector options and env
  names for the picker come from the same `Project` calls `select_option`
  and `pick_env` use.
- File access: none new. The live view and the picker write only to the
  terminal.

## 9. Docs

- `README.md`: one line for `--jobs` and `-i` beside the suite line (plain
  short copy).
- `docs/guide.md`: the suite section describes the live view, `--jobs`
  (and the race warning), `-i`, and that a pipe gets plain output.
- The skill (`skills/postui/SKILL.md`, and the `.claude/skills/postui`
  copy): `--jobs` in the usage line; without a terminal the output is the
  plain one; under a pseudo-terminal pass `--plain`; don't use `-i` (it
  needs a terminal).
- `CHANGELOG.md`: an Added entry.
- Usage text in `cli/mod.rs` (`SUITE_USAGE`).

## 10. Tests

- **Plain output unchanged:** existing `cli_run.rs`/`cli_commands.rs`
  suite tests pass as they are (the binary under test has no terminal, so
  it takes the plain path). New ones: `--jobs 3` keeps sidebar order in
  plain and JSON against a test server whose earlier requests answer
  slower; `jobs` in JSON; `jobs N` in the plain heading only when N > 1.
- **Concurrency:** with `--jobs 2` and a test server that holds requests,
  at most two are in flight at once; with the default, one.
- **Parsing:** `-j 0`, `-j x`, `--jobs` missing its value; `-i` and `-j`
  on `run`/other commands refused as before; `-i` without a terminal is
  the usage error (§4); `--plain` accepted on `suite` only.
- **Output mode choice:** a function of (stdout is a terminal, `--json`,
  `--plain`, `TERM`, `NO_COLOR`) tested as a table, so the live path's
  selection is covered without a terminal.
- **Live rows as text:** row building for each state (waiting, in flight,
  passed, failed status, `!`, skipped) at a few widths, with and without
  color (`NO_COLOR`), including the clip and the wrap of a final error.
- **Band model:** which rows are final vs in the band as results arrive
  out of order; the throttle at a given height; `… N more`.
- **Picker model:** key handling, wraparound, env change rebuilding
  selector rows, starting values from saved state and flags, cancel.
- A tmux-driven check of the real live view and picker (by hand or a
  scripted capture) before merge; the band's redraw is hard to unit-test.

## 11. Left to the builder

- Spinner frame rate and the exact tick loop (`tokio::select!` over
  results, a ~100 ms interval, and Ctrl+C).
- Exact column widths and how names that differ greatly in length are
  handled (cap the name column, clip with `…`).
- How the method hues map onto the 16 ANSI colors.
- Module layout under `cli/`.
