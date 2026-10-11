# Curl import and export — design

Date: 2026-10-11. Status: approved in chat section by section; awaiting
written-spec review.

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
  conversions (§4.3) happen silently.
- **D4 Import creates a new request** through the New request name
  prompt. No paste-into-URL-bar conversion.

Mine, approved with the design sections:

- **R1** A hand-written tokenizer (no `shlex`/`shell-words`): browsers
  emit bash ANSI-C quoting `$'…'`, which neither crate parses.
- **R2** The URL is stored verbatim, query string included — not split
  into the params table (duplicate keys and encoding would not survive
  an `IndexMap` round trip).
- **R3** Bodies are stored verbatim, never reformatted, even when they
  are not JSON. A non-JSON body with no `Content-Type` from the command
  gets curl's own default, `application/x-www-form-urlencoded`, as a
  header row — otherwise postui's auto `Content-Type: application/json`
  would change what is sent.
- **R4** Export emits one flag per line with ` \` continuations and
  POSIX single-quote escaping.
- **R5** No new keys. Palette commands and right-click rows only.

## 3. Core module: `crates/postui-core/src/curl.rs`

Pure functions; no file access, no app state.

```rust
pub struct CurlImport {
    pub request: HttpRequest,
    /// Each dropped flag as written, with its value: `-F file=@a.png`.
    pub ignored: Vec<String>,
    /// `{METHOD} {last non-empty path segment, else host}`, never
    /// containing `/` (a `/` in a display path means a folder).
    pub suggested_name: String,
}

pub fn parse(text: &str) -> Result<CurlImport, CurlError>;
pub fn to_curl(req: &PreparedRequest) -> String;
```

`CurlError` variants each carry a short toast message: not a curl
command, unclosed quote, no URL, Windows cmd-style quoting ("copy as
cURL (bash) instead"), a flag missing its value.

### 3.1 Tokenizer (private)

POSIX shell words: whitespace separates; `'…'` literal; `"…"` with
`\"`, `\\`, `\$`, `` \` `` and backslash-newline escapes; bare `\x`
escapes `x`; backslash-newline outside quotes is a continuation;
`$'…'` with the ANSI-C escapes `\n \t \r \\ \' \" \xHH \uHHHH \0NNN`.
Adjacent quoted and bare pieces join into one word. A leading `$ ` or
`% ` prompt is stripped; the first word must be `curl`. Detected cmd
style (`^"` or a trailing `^` continuation) is `CurlError`, not a
garbled import.

## 4. Import

### 4.1 Entry points

- Palette: **"Import curl from clipboard"** — imports into the space
  root.
- Sidebar folder right-click menu: **"Import curl here…"**, after
  "New request here…".

Both dispatch `Action::ImportCurl { folder: Option<String> }`. Both
need a project (`refuse_without_project`, like New request).

### 4.2 Flow

1. Read the clipboard. Unreadable, or not starting with `curl` after
   leading whitespace and an optional prompt → error toast "clipboard
   doesn't hold a curl command". Nothing else happens.
2. `curl::parse`. A `CurlError` → error toast with its message.
3. Push the New request prompt (title "Import curl") with
   `PromptKind::ImportCurl { request, ignored }`, prefilled with
   `folder/` (when given, space segment stripped as `PromptNewRequestIn`
   does) plus `suggested_name`.
4. Enter → `Project::create_request(display_path, request)`: the same
   validation, sibling uniqueness, slug derivation and undo entry as a
   new request; the same "name taken"/"empty name" errors keep the
   prompt open. On success the request is opened and, when `ignored`
   is non-empty, a warning toast reads
   `imported; ignored: -F file=@a.png, --compressed`. One undo removes
   the request. Esc abandons with nothing created.

### 4.3 Flag mapping

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
- Each row of §4.3, the `ignored` list contents and order, the
  suggested name, every `CurlError`.
- Export quoting: `'` in a header and body, a body starting with `@`,
  HEAD, GET with a stored body, insecure.
- Round trip: for representative requests,
  `parse(to_curl(prepare(r)))` yields the prepared method, URL,
  headers (as a set of rows), body and insecure flag.

App (`app/tests.rs`):

- Palette and folder-menu import: prefill, create, open, ignored-flag
  toast, one undo removes it, Esc creates nothing, name-taken keeps the
  prompt.
- Copy as curl for the open request with unsaved edits and for a
  closed request; the unresolved-variable refusal; the secret note.
- Menu rows present and ordered (request: after Duplicate; folder:
  after New request here…).

Manual: both entry points driven in a real terminal through tmux.
