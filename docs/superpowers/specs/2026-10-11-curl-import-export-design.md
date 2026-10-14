# Curl import and export — design

Date: 2026-10-11. Status: approved in chat section by section, then
reviewed in chat and approved (D5–D7 and R6–R8 added in that review).

## 1. Purpose

Bring requests saved elsewhere into postui, and hand requests to other
people, through the lingua franca every HTTP tool and README speaks: a
`curl …` command. Success: a command copied from a browser's devtools
("Copy as cURL") or a README becomes a working request; a request
becomes a command a teammate can run in a shell and get the same call.

Out of scope: Postman, OpenAPI, HAR and other formats; files as a
transport; Windows `cmd` quoting on export; growing the request model
(raw/form/multipart bodies, arbitrary methods).

## 2. Decisions

The user's, from the brainstorm (2026-10-11):

- **D1 Transport: clipboard only.** Import reads the clipboard; export
  writes it. No file import or export.
- **D2 Export is fully resolved.** The command is exactly what Send
  would put on the wire for the active environment, secrets included.
- **D3 Unrepresentable flags: import, warn.** Everything that maps is
  imported; each dropped flag is named in a warning toast. Lossless
  conversions (§4.4) happen silently.
- **D4 Import creates a new request** through the New request name
  prompt. No paste-into-URL-bar conversion.
- **D5 A JSON body with no `Content-Type` sends as JSON** (spec review).
  Real curl would send `-d '{"a":1}'` form-urlencoded; the import keeps
  postui's auto `application/json` instead, because that is what the
  author meant. Only a body that does not parse as JSON gets curl's form
  default (R3).
- **D6 Several commands import as several requests** (spec review). A
  clipboard holding more than one curl command imports all of them, one
  name prompt per command in sequence; Esc on a prompt skips that
  command (§4.2).
- **D7 Shell variables become postui variables** (spec review). `$NAME`
  and `${NAME}` outside single quotes become `{{NAME}}`; a body that had
  one gets `substitute_body` switched on (§3.2).
- **D10 A dropped body keeps curl's method** (code review, 2026-10-11).
  A body flag the import cannot carry (`-d @file`, `--json @file`,
  `--data-urlencode name@file`, `-F`/`--form`, `--form-string`, `-T`)
  is listed as ignored, yet the method is still the one curl would
  send — POST, or PUT for `-T` — unless `-X`, `-I` or `-G` says
  otherwise. The request reaches the same endpoint, with no body.
- **D18 A `# @name` comment names the import** (task #2, 2026-10-14).
  A comment line `# @name List users` above a curl command prefills
  its name prompt with `List users` instead of the URL's suggestion, as
  in VS Code REST Client and JetBrains `.http` files. Other comments
  stay ignored. Import only: Copy as curl writes no name. (The details
  in §3.1 and R8 are mine.)

Mine, approved with the design sections:

- **R1** A hand-written tokenizer (no `shlex`/`shell-words`): browsers
  emit bash ANSI-C quoting `$'…'`, which neither crate parses.
- **R2** The URL is stored verbatim, query string included — not split
  into the params table (duplicate keys and encoding would not survive
  an `IndexMap` round trip).
- **R3** Bodies are stored verbatim, never reformatted, even when they
  are not JSON. A body that does not parse as JSON (`serde_json`) and
  has no `Content-Type` from the command gets curl's own default,
  `application/x-www-form-urlencoded`, as a header row — otherwise
  postui's auto `Content-Type: application/json` would change what is
  sent. A body that parses as JSON gets no row (D5).
- **R4** Export emits one flag per line with ` \` continuations and
  POSIX single-quote escaping.
- **R5** No new keys. Palette commands and right-click rows only.

Mine, proposed in the spec review (user may override):

- **R6** A parse error in any command of a multi-command paste imports
  nothing: a broken quote can swallow the commands after it, so a
  partial import would be guesswork.
- **R7** (user's decisions, 2026-10-12: values go in as secrets in the
  active environment; a value is literal text only, and anything else
  is no value; every variable the curl uses is declared; lines other
  than curl commands and assignments are ignored. The details are mine.)
  - *Which lines* (user's decisions, 2026-10-12: anything complicated
    is skipped with no value; then, after the ninth code review, all or
    nothing: a paste with any skipped line gives no value at all). An
    assignment is a command alone on its line (nothing before it, ended
    by the line's end, not `;`, `&`, `&&` or `||`, no pipe or redirect;
    a line ending in `&&`, `||` or `|` goes on into the next, as in
    bash) made only of unquoted `NAME=value` words, after an optional
    `export`: read, not skipped. Any other command that is not a curl
    command (`echo`, `cd`, `export T=1 &`, `[ -n "$CI" ] && export T=1`,
    `if …; then`, `fi`, `f() {`, `unset T`, `export T`) is counted in
    `skipped_lines`, and when there is any, every variable of every
    command has no value, wherever the skipped line sits: a block or
    condition around a plain-looking assignment is not read, so no
    value is trusted (`CurlBatch::values_dropped` says one was
    dropped; the summary says so). Assignments leading a curl command
    (`TOKEN=abc curl …`, `TOKEN+=abc curl …`) do not hide it: bash
    expands curl's words before they take effect and they do not
    outlive the command, so they change nothing and are listed in
    `ignored` as `TOKEN=…` or `TOKEN+=…` (no value, which may be a
    secret).
  - *Which values.* A variable's value is the one the paste's last
    assignment before that curl command gave it, when that is literal
    text, quoted or not: nothing bash would expand (`$` outside single
    quotes, backticks, an unquoted `~` or `{`) and no `{{`, which postui
    would read as a variable. `$'…'` and `$"…"` are quotes. Any other
    value (`$(…)`, `"$BASE/v1"`, `~/k`) is no value; so is a variable
    the paste never assigned.
  - *What lands.* Every variable a created request's `{{NAME}}` came
    from is declared as a secret variable in the request's own undo
    step: with its value in the active environment's secrets
    (`.local/secrets.toml`, out of the project files) when it has one,
    with none when not (Send then reports it missing until someone
    fills it in; the summary says "declared X as a secret with no
    value"). A value an earlier, deleted declaration of the name left in
    `.local/secrets.toml` is dropped from every environment in the same
    step, so the secret holds only what the paste gave. A name the
    project already has (a
    declared variable, selector or field, or a value some environment
    file sets without declaring it, which Send reads there) is left
    exactly as it is and reported as kept; an environment file that
    can't be read refuses the create, since what it sets is unknown. A
    reserved name (`options`, `groups`, `entries`, `selectors`) is never
    saved, and the summary says so. When a later command of the batch
    gave a variable an earlier one saved a different value, or declared
    with no value, the later request sends what the first got, and the
    summary says so. Undo of the
    create removes the declarations and values with the request.
- **R8** A suggested name already taken (in the folder, or by an earlier
  import of the same batch) prefills as `GET users 2`, `GET users 3`, ….
  A `# @name` with folders in it (`api/ping`, D18) numbers its leaf in
  the folder it points into: `api/ping 2` (2026-10-14).

## 3. Core module: `crates/postui-core/src/curl.rs`

Pure functions; no file access, no app state.

```rust
pub struct CurlImport {
    pub request: HttpRequest,
    /// Each dropped flag as written, with its value: `-F file=@a.png`;
    /// also a dropped pipeline tail (`| jq .`) or redirect (`> out.json`),
    /// and `$(…)`/backtick text kept literal.
    pub ignored: Vec<String>,
    /// Shell variables converted to `{{NAME}}`, in first-seen order.
    pub converted: Vec<String>,
    /// One per `converted` variable, with its value (R7); `None` for no
    /// value.
    pub variables: Vec<(String, Option<String>)>,
    /// A `# @name` comment's text as written (D18; a `/` in it means a
    /// folder, as typed into the prompt), else `{METHOD} {last non-empty
    /// path segment, else host}`, never containing `/`.
    pub suggested_name: String,
}

pub struct CurlBatch {
    /// One per curl command, in paste order; never empty.
    pub commands: Vec<CurlImport>,
    /// Non-comment, non-blank lines that were neither curl commands nor
    /// assignments (R7). Any at all: no variable has a value.
    pub skipped_lines: usize,
    /// A value was given, and a skipped line dropped it (R7).
    pub values_dropped: bool,
}

pub fn parse(text: &str) -> Result<CurlBatch, CurlError>;
pub fn to_curl(req: &PreparedRequest) -> String;
```

`CurlError` variants each carry a short toast message: not a curl
command (no curl command anywhere in the text), unclosed quote, no URL,
Windows cmd-style quoting ("copy as cURL (bash) instead"), a flag
missing its value. In a multi-command paste the message is prefixed
with the command's position: `command 2: unclosed quote` (R6).

### 3.1 Tokenizer (private)

POSIX shell words: whitespace separates; `'…'` literal; `"…"` with
`\"`, `\\`, `\$`, `` \` `` and backslash-newline escapes; bare `\x`
escapes `x`; backslash-newline outside quotes is a continuation;
`$'…'` with the ANSI-C escapes `\n \t \r \\ \' \" \xHH \uHHHH \0NNN`.
Adjacent quoted and bare pieces join into one word. Detected cmd style
(`^"` or a trailing `^` continuation) is `CurlError`, not a garbled
import.

Commands: an unquoted line break (not a continuation), `;`, `&&`,
`||` or a lone `&` ends a command. A line ending in `&&`, `||` or `|`
goes on into the next, as in bash. An unquoted `|` or a redirect (`>`, `>>`, `<`,
`2>`, …) ends the curl part of a command; the rest up to the command's
end is listed in `ignored`. `#` at the start of a word starts a comment
to the end of the line. A comment that is its own line and reads
`# @name <text>` (D18; `#@name` too, the text trimmed, never empty)
names the next command, with comment and blank lines between allowed;
a later one before that command replaces it, and whatever command comes
next takes it, curl or not (2026-10-14). A leading `$ ` or `% ` prompt on a command is
stripped. An assignment line (`export TOKEN=…`, `BASE=…`, alone on
its line) is read (R7). A command whose first word after any leading assignments is
`curl` is a curl command. Any other command (an `echo`, a `cd`) is
skipped and counted in `skipped_lines`; comments and blank lines are
not counted.

### 3.2 Shell variables (D7)

Outside single quotes (bare words, `"…"`), `$NAME` and `${NAME}` (NAME
= `[A-Za-z_][A-Za-z0-9_]*`, always a valid postui variable name) become
`{{NAME}}` and NAME joins `converted`. `${NAME:-x}` and other operator
forms, `$1`, `$?`, `$$`, `$@`, and a lone `$` stay literal text.
Command substitution `$(…)` and `` `…` `` stays literal and is listed in
`ignored` as `$(…) kept as text`. Inside `'…'` and `$'…'` everything is
literal, as in bash. When the body holds a converted variable, the
request gets `substitute_body = true`.

## 4. Import

### 4.1 Entry points

- Palette: **"Import curl from clipboard"** — imports into the space
  root.
- Sidebar folder right-click menu: **"Import curl here…"**, after
  "New request here…".

Both dispatch `Action::ImportCurl { folder: Option<String> }`. Both
need a project (`refuse_without_project`, like New request).

### 4.2 Flow

1. Read the clipboard. Unreadable → error toast "clipboard doesn't hold
   a curl command". Nothing else happens.
2. `curl::parse`. A `CurlError` (including no curl command anywhere) →
   error toast with its message. Nothing is created (R6).
3. Push the New request prompt with
   `PromptKind::ImportCurl { pending, folder, created, ... }` for the
   first command: title "Import curl" (one command) or
   "Import curl (1 of 3)", prefilled with `folder/` (when given, space
   segment stripped as `PromptNewRequestIn` does) plus the command's
   `suggested_name` (its `# @name`, D18), made unique per R8. Under the field, muted: the
   command's method and URL, and `ignores …` with its ignored list when
   there is one. In a batch the cancel button reads "Skip" and the
   footer says `esc skip`, since it only skips this command (added
   2026-10-12). Before the first prompt, unsaved work in the editor
   meets the usual dirty gate once; its Discard discards there and
   then (a scratch request becomes a blank one), so skipping every
   prompt afterwards still leaves what the answer said; undo brings it
   back (2026-10-12).
4. Enter → `Project::create_request(display_path, request)`: the same
   validation, sibling uniqueness, slug derivation and undo entry as a
   new request; the same "name taken"/"empty name" errors keep the
   prompt open. Success creates the request (its own undo entry) and
   moves to the next command's prompt. Esc skips the current command
   and moves on.
5. After the last prompt: the last request created is opened, and one
   summary toast reports the batch (§4.3). Nothing created → no toast,
   nothing opened.

### 4.3 Summary toast

Built from what was created; omitted parts are omitted:

- One command, nothing to report: no toast beyond opening it.
- `imported 2 of 3` when more than one command, or any was skipped.
- `; ignored: GET users: --compressed, -L` — per created request with a
  non-empty `ignored` (name prefix only in a batch).
- `; converted $TOKEN → {{TOKEN}}` — the union of `converted`.
- `; saved TOKEN as a secret in dev` (`as secrets` for several; no
  `in …` with no environment active) — R7's saved values.
- `; declared TOKEN as a secret with no value` (`as secrets` for
  several) — R7's variables with no value; Send reports them missing
  until filled in. Makes the toast a warning.
- `; kept the existing base` — variables the project already had,
  less any this import saved, with the paste's value in the active
  environment.
- `; kept the existing base, not the paste's value` — the same, when the
  project's value in the active environment (or its lack of one) is not
  the paste's: the request sends something else. Makes the toast a
  warning (after the third 2026-10-12 code review).
- `; kept the first value of T` — names this import saved that a later
  command gave another value (R7); makes the toast a warning.
- `; did not save a later value of T` — names this import declared with
  no value that a later command gave one; makes the toast a warning.
- `; did not save entries (a reserved name)` (`reserved names` for
  several) — assigned names no variable may have; the request leaves
  them unresolved. Makes the toast a warning.
- `; saved no values: the paste has other shell lines` when
  `values_dropped` (R7).
- `; skipped 1 non-curl line` when `skipped_lines > 0`.

Warning kind when anything was ignored or skipped, success otherwise.

### 4.4 Flag mapping

| curl | postui |
|---|---|
| URL (positional or `--url`); the last one wins, earlier ones listed as ignored | `url`, verbatim |
| `-X`/`--request M` | `method` when M is one of the seven (case-insensitive); otherwise ignored with a warning and curl's rule applies (POST with data, else GET) |
| `-I`/`--head` | HEAD |
| `-H`/`--header 'K: V'` | header row, in order; a repeated name joins values (`; ` for `Cookie`, `, ` otherwise). `K;` (curl's empty-value form) → empty value. `K:` with nothing (curl's remove-header form) → ignored |
| `-d`/`--data`/`--data-ascii`/`--data-binary`/`--data-raw` | body text; several join with `&`. Data without `-X` implies POST. `-d @file`/`--data-binary @file` → ignored (no file access), still POST (D10); `--data-raw @x` is literal |
| `--data-urlencode` | `name=content` → `name=` + percent-encoded content; bare `content` → encoded content; `@file` forms ignored; joins with the other data by `&` |
| `--json D` | body D, plus `Content-Type: application/json` and `Accept: application/json` unless the command set them. As curl does, a `--json` piece joins the data before it with no `&` |
| `-G`/`--get` | the joined data is appended to the URL's query (`?` or `&`, ahead of any `#fragment`) instead of becoming the body; method GET unless `-X`/`-I` said otherwise |
| `-u`/`--user u:p` | `Authorization: Basic base64(u:p)` unless an explicit Authorization header is given. With `--digest`, `--ntlm`, `--ntlm-wb`, `--negotiate`, `--anyauth` or `--aws-sigv4` (curl would not send Basic) the flag is ignored |
| `-b`/`--cookie` containing `=` | `Cookie` header (joined like a repeated Cookie header); without `=` it names a cookie file → ignored |
| `-A`/`--user-agent`, `-e`/`--referer` | `User-Agent` / `Referer` header |
| `-k`/`--insecure` | `insecure = true` |
| `-F`/`--form`, `--form-string`, `-T`/`--upload-file` | ignored; method POST (`-T`: PUT) unless `-X`/`-I`/`-G` said otherwise (D10) |
| anything else (`--compressed`, `-L`, `-o`, `-s`, `-v`, `-i`, `--max-time`, …) | ignored; a flag that takes a value consumes it and both are listed |

Short flags may be bundled (`-sSL`, `-sk`) and a short flag's value may
be attached (`-XPOST`, `-HAccept:x`); long flags accept `--flag=value`.
The table of value-taking curl flags covers the common set so an
unknown flag's value is not mistaken for the URL; an unknown flag not
in the table is assumed valueless and listed.

Body storage: `Body::Json { text }` verbatim (R3). `substitute_body`
stays off; `{{…}}` in imported text is ordinary text.

## 5. Export

### 5.1 Entry points

- Palette: **"Copy as curl"** — the open request **as it stands in the
  editor, unsaved edits included**.
- Sidebar request right-click menu: **"Copy as curl"**, directly after
  "Duplicate" (away from Delete). A row that is the open request uses
  the editor's live state; any other row uses its saved file, loaded
  through `Project` without opening it. Broken rows don't offer it.

Both dispatch `Action::CopyAsCurl(Option<String>)` (`None` = open
request). No open request for the palette form → warning toast
"no request open".

### 5.2 Resolution

The same `PrepareContext` Send builds (active environment, default
headers, TLS override) and `prepare()`. So the command carries the
substituted URL with merged params, inherited default headers, the auto
`Content-Type`, the body (none for GET/HEAD; substituted when
`substitute_body`), and `-k` when the prepared request is insecure.
Headers the HTTP client adds itself are not included. `prepare()`
returning `Unresolved` → the same error toast Send shows; nothing is
copied.

### 5.3 Format (`to_curl`)

```
curl -X POST 'https://api.example.com/users?page=2' \
  -H 'Authorization: Bearer abc123' \
  -H 'Content-Type: application/json' \
  --data-raw '{"name": "x"}'
```

- GET omits `-X`; HEAD is `--head`; other methods `-X M`.
- `-k` follows the URL when insecure.
- Headers in prepared order; body as `--data-raw` (never `@`-read).
- Every argument single-quoted; `'` → `'\''`. Line breaks inside a body
  stay literal inside the quotes.

### 5.4 Copy and toast

Through the existing clipboard tiers (`Clipboard::copy`, including its
size-threshold warning). Success toast "Copied as curl"; when any
variable with `VarMeta::Secret` was substituted into the command, it
reads "Copied as curl — includes secret values". Informational only
(D2).

## 6. Errors

Every failure is a toast and changes nothing: clipboard unreadable or
not curl, parse error, unresolved variables, clipboard copy failure. An
import never partially creates a request.

## 7. Testing

Core (`curl.rs` tests):

- Real "Copy as cURL" samples from Chrome, Firefox and Safari, including
  `$'…'` bodies with `\n`, `\'` and `\u` escapes; README one-liners;
  multi-line `\` continuations; bundled and attached short flags;
  `--flag=value`.
- Each row of §4.4, the `ignored` list contents and order, the
  suggested name, every `CurlError`; D5's JSON/non-JSON `Content-Type`
  split.
- Commands (§3.1): splitting on line breaks, `;`, `&&`, `||`; pipeline
  tails and redirects listed; comments; skipped non-curl lines counted;
  a parse error in command 2 fails the whole batch with its position.
- Shell variables (§3.2): `$X`, `${X}` in bare and double-quoted text
  convert; single-quoted and `$'…'` stay literal; operator forms,
  specials and `$(…)` stay literal; a body variable sets
  `substitute_body`.
- Export quoting: `'` in a header and body, a body starting with `@`,
  a `$` in a URL or body (single quotes keep it literal on re-import),
  HEAD, GET with a stored body, insecure.
- Round trip: for representative requests,
  `parse(to_curl(prepare(r))).commands[0]` yields the prepared method,
  URL, headers (as a set of rows), body and insecure flag.

App (`app/tests.rs`):

- Palette and folder-menu import: prefill, create, open, summary toast,
  one undo removes it, Esc creates nothing, name-taken keeps the prompt.
- Batch import: "(1 of 3)" titles, Esc skips one, the last created is
  opened, each request its own undo entry, R8 unique prefills across the
  batch, the summary toast.
- Copy as curl for the open request with unsaved edits and for a
  closed request; the unresolved-variable refusal; the secret note.
- Menu rows present and ordered (request: after Duplicate; folder:
  after New request here…).

Manual: both entry points driven in a real terminal through tmux.
