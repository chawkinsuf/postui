# CLI and skill fixes from the Ollama dogfood — Design

Task #219. Written 2026-10-30, after Claude used the installed skill to build
out the Ollama requests in the user's default project and reported what got
in the way. Builds on `2026-10-28-cli-for-agents-design.md` (#103), whose
rules it amends where noted.

## 1. Findings

- **A value holding `{{name}}` is sent with its braces.** With
  `ollama-url = "https://{{ollama-host}}"` (a default in `variables.toml`),
  `run --dry-run` printed `GET https://{{ollama-host}}/api/version` and
  `check` passed. `prepare` fills each `{{name}}` of the request once; a
  value's own tokens are never filled in, and nothing says so.
- **`run` ignores the request's saved jq view.** Every request with a
  `[[jq_view]]` needed its filter typed again as `--jq`, and without one an
  embeddings body printed 19 KB of floats.
- **No way to change a variable for one run.** Only `--select` overrides
  anything, so a request acting on another model needed its file changed.
- **`edit end` refused over an error that was there before the edit**
  (an undeclared `{{user}}` in a vllm request), and the skill had not said
  to look first.
- **`run` prints a report, not the body.** The request line, root, file,
  environment, request headers, status line and response headers come
  before the body every time; most runs were piped through grep or sed to
  get the body back.

## 2. Rules

Source: `you` = the user's own words; `mine (approved)` = my proposal the
user picked; `mine` = my call, open to veto (§6).

| # | Rule | Source |
|---|------|--------|
| D1 | A `{{name}}` in a variable's value is not filled in (values still aren't templates). When a token the **URL or a query parameter** uses has such a value, the request is not sent: `prepare` refuses it, so the TUI's Send and Copy as curl refuse too, and `check` reports an error. | you ("if the url specifically has an unset var maybe we shouldnt even send the request?"), mine (approved) (the URL refuses, the rest warns) |
| D2 | When a **sent header** (its name or value) or a **substituted body** uses such a value, the request is sent as written, with a warning: the TUI's send toast, `run`'s warnings, and a `check` warning. | mine (approved) |
| D3 | `run --view` filters the body with the request's own jq filter, the one its response pane applies: the current view's, else the unnamed (scratch) filter. `--view=NAME` uses the saved view NAME. | you ("lets add --view"), mine (the two forms) |
| D4 | `run --var NAME=VALUE` (repeatable) and `suite --var` give a variable a value for this run only. It wins over every other source, the request's own `[variables]` included, and is never written anywhere. | you ("add it"), mine (precedence, suite) |
| D5 | `run` prints only the body by default (or `--jq`/`--view`'s results), like curl. `-i`/`--include` prints the status and response headers before it; `-v`/`--verbose` prints the whole report `run` prints today. | you ("it should probably default to just the body as output and have --header options for more info") , mine (the flag names and what each shows) |
| D6 | By default, what doesn't go to stdout but matters goes to stderr, each line `postui: …`: a status of 400 or more (`postui: 404 Not Found`), a send or prepare error, a jq error, a failed `-o` write, and warnings. | mine (approved) (status on stderr), mine (the rest) |
| D7 | The skill says to run `check` before changing a project, so errors that were already there are known; when `edit end` refuses, the agent asks the user, offering `--force` as one answer. | you ("its fine to just ask the user what to do with --force as an option. but yes, run the check before changes to check") |
| D8 | The skill and `reference.md` say what changed: the default output and `-i`/`-v`, `--view`, `--var`, that a value's `{{name}}` isn't filled in, and `suite --timeout` (missing from the skill's synopsis). | you ("update the skill as needed") |

Dropped from the findings: `postui projects` printing `(none)` when the
config dir can't be read (you: that is the same as having no config dir),
and a selector default option (you: "i guess i was thinking of the default
per env", which a plain variable with a per-environment value covers).

## 3. Design

### D1, D2: a value holding a token (`postui-core::prepare`)

One helper finds, for a template, the first token whose value holds a
token of its own: `(var, token)`. `prepare` asks it about:

- the URL and each enabled parameter's key and value: the first hit is
  `PrepareError::ValueHoldsVariable { var, token }`, after the unresolved
  check (an unresolved name is reported first, as today);
- each **sent** header row's name and value (defaults and the request's
  own, after the replaced/duplicate/client-owned rules) and, with
  `substitute_body`, the body: each hit is
  `PrepareWarning::ValueHoldsVariable { var, token, place }`.

`PrepareError` gains its second variant. Its consumers: the TUI's Send and
Copy as curl show its text as a toast, as they show an unresolved one;
`check` reports it as an error and reports the new warning (only this
warning kind: the duplicate-default one would repeat on every request);
`run`/`suite` report it as a refusal. The texts (mine):

- error: `ollama-url's value holds {{ollama-host}}, and postui doesn't fill in variables inside a value`
- warning: `the Authorization header: token's value holds {{x}}, which is sent as written`

### D3: `--view`

`--view` takes no value; `--view=NAME` takes one after `=` only, so a slug
after `--view` is never read as a view name (as `--keydump[=BITS]`). The
filter comes from the request as read: `jq_current`'s view, else the scratch
`jq`. Whether the pane's switch is off (`jq_enabled = false`) doesn't
matter: asking for the view is asking for its filter (mine). A request with
no filter, or no view of that name, is an error naming the request's views,
exit 2. `--view` with `--jq`, or with `--dry-run`, is a usage error. From
there it is `--jq` with that filter: same output, same `-o` and errors.

### D4: `--var`

`RunOpts` gains `vars: Vec<(String, String)>`. The value goes into the
request's `[variables]` in memory (enabled), after the request is read and
before `prepare`, which already ranks those first; nothing in core changes.
A name given twice, or not a valid variable name, is a usage error. A name
that `variables.toml` doesn't declare (as a variable or a selector field)
and that no request of the run has in its `[variables]` is an error naming
it, exit 2: a typo would otherwise do nothing (mine). A `--var` for a
secret is masked on the request side like the secret, and its value joins
the output mask (mine).

### D5, D6: `run`'s output

`RunReport` is unchanged; only its text has three forms.

| Mode | stdout | stderr |
|---|---|---|
| default | the body as it came (masked), or the jq results one per line; nothing with `-o` | `postui: …` lines (D6) |
| `-i` | `200 OK  176 ms (first byte 135 ms), 3497 bytes`, then each response header as `name: value`, a blank line, then the body or results | errors and warnings as default |
| `-v` | today's report, unchanged | nothing new |
| `--dry-run` | today's report (the request side is what a dry run is for) | nothing new |
| `--json` | unchanged | unchanged |

Exit codes don't change. `suite` doesn't change. The reason phrase comes
from the status code (`404 Not Found`); a code with none prints bare.

## 4. Tests

| Rule | Test |
|---|---|
| D1 | `prepare` refuses a URL token, a param key and a param value whose value holds a token, naming the var and token; an unresolved name is still reported first; the TUI's Send toasts the text and sends nothing; `check` reports an error naming the request file. |
| D2 | A sent request header, a sent default and a substituted body each warn; an unsent (replaced) default and an unsubstituted body don't; `check` reports the warning; `run` sends. |
| D3 | `--view` uses `jq_current`'s filter, else the scratch; `--view=NAME` a named view (name matched as views are); no filter, an unknown name, `--view` with `--jq` or `--dry-run` are errors. |
| D4 | `--var` beats the env, a selection and the request's own `[variables]`; `suite --var` reaches every request; a name given twice, an invalid one and an undeclared one are errors; a secret given with `--var` is masked. |
| D5 | Default prints only the body (and only the results with `--jq`); `-i` prints status, headers, blank line, body; `-v` prints today's report; `--dry-run` prints today's report. |
| D6 | A 404 prints the body on stdout and `postui: 404 Not Found` on stderr, exit 1; a prepare refusal, a send error and a jq error go to stderr; warnings go to stderr. |

## 5. Layers

- `postui-core::prepare`: the helper, `PrepareError::ValueHoldsVariable`,
  `PrepareWarning::ValueHoldsVariable`.
- `postui`: the TUI's two `PrepareError` matches; `cli/mod.rs` (flags,
  usage); `cli/run.rs` (`--view`, `--var`, the output modes); `cli/suite.rs`
  (`--var`); `cli/check.rs` (the new error and warning).
- `skills/postui/SKILL.md`, `skills/postui/reference.md`.

## 6. My calls, for veto

1. `--view=NAME` only with `=`; bare `--view` is the pane's filter, even
   when its switch is off.
2. `--var` must name a declared variable or selector field, or a name in a
   run request's `[variables]`; it beats the request's own `[variables]`.
3. A `--var` given for a secret joins the output mask.
4. `-i`/`--include` and `-v`/`--verbose` as the flag names (curl's), with
   `-i` printing headers as `name: value`.
5. `--dry-run` keeps printing the full report.
6. `check` reports only the new prepare warning, not the existing two.
7. D1 refuses only for the URL and query parameters; a token in a header
   value or a body might be meant literally.
