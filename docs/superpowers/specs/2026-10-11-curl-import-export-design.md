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
- **R7** `NAME=value` / `export NAME=value` lines are skipped like any
  other non-curl line — they do not fill in the variable. `{{NAME}}`
  stays undefined until the user defines it, and Send says so.
- **R8** A suggested name already taken (in the folder, or by an earlier
  import of the same batch) prefills as `GET users 2`, `GET users 3`, ….

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
    /// `{METHOD} {last non-empty path segment, else host}`, never
    /// containing `/` (a `/` in a display path means a folder).
    pub suggested_name: String,
}

pub struct CurlBatch {
    /// One per curl command, in paste order; never empty.
    pub commands: Vec<CurlImport>,
    /// Non-comment, non-blank lines that were not curl commands.
    pub skipped_lines: usize,
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

Commands: an unquoted line break (not a continuation), `;`, `&&` or
`||` ends a command. An unquoted `|` or a redirect (`>`, `>>`, `<`,
`2>`, …) ends the curl part of a command; the rest up to the command's
end is listed in `ignored`. `#` at the start of a word starts a comment
to the end of the line. A leading `$ ` or `% ` prompt on a command is
stripped. A command whose first word is not `curl` (an `echo`, an
`export TOKEN=…`, a `cd`) is skipped and counted in `skipped_lines`
(R7); comments and blank lines are not counted.

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
   `suggested_name`, made unique per R8.
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
- `; skipped 1 non-curl line` when `skipped_lines > 0`.

Warning kind when anything was ignored or skipped, success otherwise.

### 4.4 Flag mapping

| curl | postui |
|---|---|
| URL (positional or `--url`); the last one wins, earlier ones listed as ignored | `url`, verbatim |
| `-X`/`--request M` | `method` when M is one of the seven (case-insensitive); otherwise ignored with a warning and curl's rule applies (POST with data, else GET) |
| `-I`/`--head` | HEAD |
| `-H`/`--header 'K: V'` | header row, in order; a repeated name joins values (`; ` for `Cookie`, `, ` otherwise). `K;` (curl's empty-value form) → empty value. `K:` with nothing (curl's remove-header form) → ignored |
| `-d`/`--data`/`--data-ascii`/`--data-binary`/`--data-raw` | body text; several join with `&`. Data without `-X` implies POST. `-d @file`/`--data-binary @file` → ignored (no file access); `--data-raw @x` is literal |
| `--data-urlencode` | `name=content` → `name=` + percent-encoded content; bare `content` → encoded content; `@file` forms ignored; joins with the other data by `&` |
| `--json D` | body D, plus `Content-Type: application/json` and `Accept: application/json` unless the command set them |
| `-G`/`--get` | the joined data is appended to the URL's query (`?` or `&`) instead of becoming the body; method GET unless `-X`/`-I` said otherwise |
| `-u`/`--user u:p` | `Authorization: Basic base64(u:p)` unless an explicit Authorization header is given |
| `-b`/`--cookie` containing `=` | `Cookie` header (joined like a repeated Cookie header); without `=` it names a cookie file → ignored |
| `-A`/`--user-agent`, `-e`/`--referer` | `User-Agent` / `Referer` header |
| `-k`/`--insecure` | `insecure = true` |
| anything else (`-F`, `--compressed`, `-L`, `-o`, `-s`, `-v`, `-i`, `--max-time`, …) | ignored; a flag that takes a value consumes it and both are listed |

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
