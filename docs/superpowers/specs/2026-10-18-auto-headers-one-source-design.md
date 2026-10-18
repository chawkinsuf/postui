# Auto headers from one source (task #21)

Task #21 (tidy): work out the headers a send carries in one place, used
both by the editor's display and by the send itself. It comes before #20
(promote an auto header, strike through overridden ones) and #22 (a
header the client adds on its own, such as URL userinfo's
`Authorization`, belongs in the auto section).

## 1. Findings

Four places work out the headers today, each with its own rules:

| Where | What it decides |
|---|---|
| `prepare()` (core `prepare.rs`) | Wire rows: the default-header merge, request rows, auto `Content-Type`, plus a masked twin list (`display_headers`). |
| `computed_headers()` (core `prepare.rs`) | The editor's auto section: the same merge again, plus client rows `Accept`, `Host` and `Content-Length`, each derived its own way. |
| `SentHeader::wire()` (`postui/src/http.rs`) | The response's sent section: prepared rows plus client rows, derived a third way. |
| `Editor::inherited_header_lines()` | The `✓ name value (project/overridden/disabled)` lines above the Headers table, comparing raw, unsubstituted names. |

A probe on main (a temporary test comparing the auto section with the
sent section for the same request) found four cases where they disagree:

| Request | Auto section | Sent (wire) |
|---|---|---|
| `https://Example.com:443/p` | `Host: Example.com:443` | `host: example.com` |
| `https://u:pw@example.com/p` | `Host: u:pw@example.com` | `host: example.com` |
| a request row `Host: other.test` | its row **and** the client's `Host: example.com` | only `other.test` |
| a request row `Content-Length: 2`, body `{}` | its row **and** the client's `Content-Length: 2` | only the request's row |

Spec stage7 §6 says the auto section shows "exactly what `prepare` + the
HTTP client will send", so in all four the auto section is wrong.

Two more findings:
- The editor works the auto section out twice per frame: once masked
  and once unmasked, just to decide whether the reveal toggle shows.
- Four places filter "auto rows" (`origin != Request`): the height math,
  the draw, `has_secret` and the copy target. The copy pill copies the
  row as drawn, so a masked secret copies the mask text.

## 2. Rules

| # | Behaviour | Who decided |
|---|---|---|
| R1 | One core function lists every header row of a send, in wire order: enabled defaults, enabled request rows, the auto `Content-Type`, then the client's `Accept`, `Host` and `Content-Length`. Each row carries its real value, its masked display, its origin, whether it is sent, and its unresolved tokens. `prepare`, the editor's auto section, the sent section and curl export all read it. | task (`you`: "calculated and applied from a single location") |
| R2 | A default row is not sent when any request row (enabled or disabled) has the same substituted name, or when an earlier default already has it. Unchanged. | stage3 §6 |
| R3 | The auto `Content-Type` and the client's `Accept` are not sent when a sent request or default row has their name. Unchanged. | stage7 §6 |
| R3a | `Host` and `Content-Length` belong to the client and can't be overridden. A request or default row with either name (matched after substitution, ignoring case) is allowed but never sent, and the send doesn't fail over it. The client's row is always sent and never struck through. This holds with no body too: a typed `Content-Length` on a GET is dropped. | `you` (the rule); "always, even with no body" is `mine (approved)` |
| R3b | An enabled request row R3a drops is drawn in the error colour, with a trailing note in the table's note slot (where the Vars tab writes "overrides …"): `not sent · the client sets Host`. A project default the rule drops is struck through in the auto section, like any other unsent default (R5). *Amended 2026-10-18 after review round 1:* the struck default also reads as an error and carries the same note, so it can't be taken for one a request row overrides (`you`, "add a note"; the reviewer raised it). | `you` ("highlight the row as an error"); the note and its wording are `mine (approved)`, landing in #21 rather than #20 |
| R4 | `Host` is the URL's authority the way the client sends it: host lowercased, default port dropped, no userinfo. On screen it shows exactly as sent unless a secret can change it, and then as the bare mask (amended after review round 8; see the §3 amendments). | stage7 §6; the derivation moves from `http.rs` |
| R5 | ~~The auto section draws every non-request row that is sent, plus every unsent **default** row (struck through). Unsent auto and client rows stay hidden.~~ *Amended 2026-10-18, task #57:* the auto section draws every non-request row, an unsent one struck through: a replaced default, auto `Content-Type`, client `Accept` or the URL's `Authorization` alike. The hide rule was mine ("today's rule kept") and wasn't what the user meant. #20 keeps only the promote button. | `you` for the rule and for `Accept`: "i set the accept header it disappeared from the auto list instead of getting a cross out", "i want the user to be able to see what defauts are being overridden". `mine`: the same strike for `Content-Type` and `Authorization`, and for a client row a project default replaces |
| R6 | The muted `✓ name value (status)` lines above the Headers table are removed. The auto section's strike-through is the one indicator. | `mine (approved)` |
| R7 | The auto section's copy pill copies the real value, masked or not, like the sent section's pill and the variable tooltip. | `mine (approved)` |
| R8 | A send fails on unresolved tokens only in rows that are sent (plus the URL, params and opted-in body). An overridden default's name no longer blocks it. | `mine` |
| R9 | The wire, the sent section and curl export change only by R3a: a typed `Host` or `Content-Length` used to go out, and is now dropped. Export still leaves out the client's rows (curl spec §5.2). | follows from R3a |

## 3. Design

**Core (`prepare.rs`)**

```rust
pub enum HeaderOrigin {
    /// `row` is the row's index in the request's header table, so the
    /// editor can mark a row R3a drops.
    Request { row: usize },
    Default,
    ContentType,
    Client,
}

pub struct HeaderRow {
    pub name: String,
    pub value: String,        // real; unresolved tokens stay literal
    pub display: String,      // secrets masked
    pub origin: HeaderOrigin,
    pub sent: bool,
    pub unresolved: Vec<String>,
}

pub fn header_rows(req: &HttpRequest, ctx: &PrepareContext) -> Vec<HeaderRow>
```

- `header_rows` replaces `computed_headers` and the merge loop inside
  `prepare`. It never fails.
- `prepare` calls it, gathers the unresolved tokens of sent rows (R8),
  and keeps the sent rows.
- `PreparedRequest.headers` becomes `Vec<HeaderRow>`: every sent row,
  the client's included, in wire order. `display_headers` goes, since
  each row carries its own display.
- `HeaderOrigin::DefaultHeader { suppressed }` and `AutoContentType` go
  too. Suppression is `sent: false` on any row, which is what #20 needs.
- The names the client owns (R3a), `host` and `content-length`, are one
  list in core. The same check marks a request or default row unsent
  and keeps the client's row sent.
- `authority()` moves from `http.rs` to core, so core gains a dependency
  on the `url` crate. It is already in the lock file (2.5.8) as
  reqwest's.

*Amended 2026-10-18 after review round 2 (`mine`):*
- `PreparedRequest` splits its rows into `headers` (what the request
  sets) and `client_headers` (what the client adds after them). The
  obvious field is then the safe one: nothing has to remember to filter
  out the client's rows before building a request or a curl command.
- `HeaderRow` gains `display_name`. On main, the auto section masked a
  secret in a header's name, and the first cut of `header_rows` lost
  that. The sent section now shows names masked too.
- A masked `Host` goes through the same parse as the real one. During
  the parse, a plain stand-in label replaces the mask, which is put back
  afterwards. The swap only happens when the URL holds the mask.
- With no secrets in play, `display` is the real text and isn't worked
  out a second time.
- A dropped typed `Host` gets no send-time warning. The reviewer raised
  it, and the user kept the Headers tab as the only signal
  (`mine (approved)`).

*Amended 2026-10-18 after review round 3:*
- `HeaderRow.sent: bool` becomes `unsent: Option<Unsent>`, which says
  why a row isn't sent: `Replaced`, `Duplicate`, or
  `ClientOwned { header, client_sends }`. The editor reads the reason
  instead of calling `client_owned` on the name again (`mine`). #20 gets
  the reason it needs to strike overridden rows.
- R3b's note names the client's row only when the client sends one:
  `not sent · the client sets Host`, else
  `not sent · set by the client only`. A GET has no `Content-Length`,
  and a `{{token}}` host has no `Host` yet. The user picked this from
  the options (`mine (approved)`).
- If the masked URL has no host, the masked `Host` falls back to the
  mask, never to the real host. A secret port, or a label pushed past 63
  characters, shows the authority as written, still masked. That
  cosmetic gap is accepted rather than chased further.

*Amended 2026-10-18 after review round 5:*
- `PreparedRequest` holds a slim `SentHeader { name, display_name,
  value, display }`. It drops the `origin`, `unsent` and `unresolved`
  fields, which meant nothing there. The response's sent section uses
  the same type (`PreparedRequest::wire_headers`), so the binary's own
  `SentHeader` goes. Proposed in response to the reviewer and approved
  by the user (`mine (approved)`).
- The masked `Host` no longer goes through the parser with a stand-in
  label. That trick drew an edge case in four rounds (round 5: an IDN
  host showed the stand-in's punycode). It now follows plain string
  rules: as written, ASCII-lowercased, default port dropped, no
  userinfo. These rules only apply when the host itself holds a secret;
  otherwise the display is the real `Host`. A non-ASCII host shows
  unpunycoded (`mine`).
- A body's substituted length is counted without building the string,
  since the Headers tab asks for it on every frame.
- The copy toast names the row as the screen shows it.
- When rows don't fit, the auto section still gives way on a short
  pane. The user ruled no change here (`you`, "No change. The issue is
  it can't scroll."). Scrolling the tab is task #56.

*Amended 2026-10-18 after review round 6:*
- Whether the masked `Host` may show the real one is now decided from
  the URL as written. If a secret's `{{token}}` sits in the scheme,
  userinfo, host or port, the real `Host` is never shown, and the
  display follows the string rules on the masked URL. Before, the
  decision came from comparing masked and real strings after
  substitution, and a secret whose value holds `/`, `?` or `#` could
  move the client's split, so part of it showed as the `Host`. This
  closes that class by construction (`mine`).
- `HeaderRow` holds its `SentHeader` and reads its fields through it,
  so the four fields and `is_masked` are written once.
- Verified but predating this branch, so recorded on #22: a typed
  `Transfer-Encoding` makes the client drop `Content-Length`, which the
  list still shows as sent.

*Amended 2026-10-18 after review round 7:*
- The round-6 rule looked for a secret only in the part of the URL
  before its path. The parser also accepts `https:/host`, which that
  split missed, so the secret showed. The rule now asks whether a
  secret's `{{token}}` is written **anywhere** in the URL; if so, the
  real `Host` is never shown. This is fail-closed, because every way of
  finding the host part by string rules has missed one of the parser's
  cases. The price is cosmetic: with a secret only in the path or
  query, an IDN host shows unpunycoded (`mine`).
- `HeaderRow` reads its `SentHeader` explicitly (`row.header.name`), not
  through `Deref`. `CLIENT_OWNED`/`client_owned` are private to core.
  The editor keeps the request table's notes ready-made, instead of
  rebuilding them each frame.
- Also verified but predating this branch, recorded on #22: a body that
  is empty on the wire sends no `content-length`, though the list shows
  `content-length: 0`.

*Amended 2026-10-18 after review round 8:* this replaces the
masked-`Host` rules from rounds 1–7. Each of those described the masked
host with rules of my own (verbatim, a parser stand-in, string rules),
and each disagreed with the parser somewhere: it leaked, or it showed a
`Host` that is never sent (`https:` for `https:/host`, `127.1` for
`127.0.0.1`, `:0443`). The new rule asks the parser a yes/no question
instead. The URL is parsed with every secret's value as `1`, as `2` and
as itself. If all three give one `Host`, no secret is in it, and it
shows exactly as sent with no reveal toggle. Otherwise it shows as the
bare mask. A host with a secret label now shows `●●●●●●` rather than
`●●●●●●.api.example.com` (`mine`). `authority` also asks the parser
first, so `https:///host` has a `Host` row like the wire does.

**Binary**
- `http::send` puts only the non-`Client` rows on the request.
  `SentHeader::wire` becomes a map over `PreparedRequest.headers`,
  lowercasing the names.
- `curl::export::to_curl` skips `Client` rows. `includes_secrets`
  becomes `url != display_url || any row's value != display`.
- `ComputedHeadersView.rows` holds only the rows the auto section
  draws, filtered once at recompute (R5). The copy index is then a plain
  `rows.get(i)`. The `has_secret` field becomes a method: any row whose
  `value != display`. That is one pass per frame instead of two.
- The draw shows `display`, or `value` once revealed, and strikes
  through when `!sent`.
- Removed: `Editor::inherited_headers`, `inherited_header_lines`, its
  layout split and height term, and `App`'s sync of the field.
- R3b: the editor keeps the request rows R3a drops, read from the rows
  it already has (`Request { row }` with `sent: false`). The table's
  `shadow` note parameter generalises to a per-row note that is either
  dim (the Vars tab's "overrides …") or error (R3b's note, the row's
  text in `theme.error`).

## 4. Tests

- Core: today's `computed_headers_*` tests move onto `header_rows`.
  New tests cover:
  - R3: a sent `Accept` default makes the client's `Accept` unsent.
  - R3a: a request or default `Host` or `{{h}}`→`Host` row, any case, is
    unsent and the client's `Host` stays sent. Likewise
    `Content-Length`, with a body and without. `prepare` succeeds and
    leaves them out.
  - R4: default port, userinfo, case.
  - The value/display pair, unsent rows present with `sent: false`, and
    R8.
- `http.rs`: `sent_headers_match_the_wire` gains a case with request
  `Host` and `Content-Length` rows, checked against a raw server: the
  wire carries only the client's. `the_auto_section_and_the_sent_section_agree`
  gains the four probe cases from §1.
- curl export: a typed `Host` row isn't exported.
- Table/editor: a request `Host` row draws in the error colour with the
  note. A disabled one doesn't, and neither does an ordinary row.
- Editor: `headers_tab_shows_inherited_rows_with_status` becomes the
  new rule: no status lines, and an overridden default is drawn once,
  struck through, in the auto section.
- App: the copy pill on a masked default row puts the real value on
  the clipboard (R7).

## 5. My choices, for veto

- R8: an overridden default's unresolved tokens no longer block a send.
- One row type, `HeaderRow`, serves both `header_rows` and
  `PreparedRequest`. A prepared request's rows always have
  `sent: true` and no unresolved tokens, so two fields are redundant
  there. A second type for so little seemed worse.
- ~~Unsent auto and client rows are in the list but hidden by the editor
  (R5), so #20's change only touches the draw.~~ Withdrawn by task #57:
  the editor draws them struck through (R5 as amended).
- `HeaderOrigin::Request { row }` carries the table index for R3b,
  rather than the editor matching rows by name or by order.
- R3b's note wording: `not sent · the client sets Host`.

## 5a. Task #22: every header the client adds (amended 2026-10-18)

#22 asked for the client's `Authorization` from URL userinfo, and any
other header the client adds or drops on its own. A raw-server probe of
every method and body shape found:

| Request | Listed before | Wire |
|---|---|---|
| `http://u:pw@host/` | nothing | `authorization: Basic dTpwdw==`, first |
| an empty body, any method that sends one | `content-length: 0` | no `content-length` |
| a typed `Transfer-Encoding`, with a body | `content-length: N` | no `content-length`; `gzip` becomes `gzip, chunked` |
| a typed `Transfer-Encoding`, no or empty body | the row, sent | dropped, and hyper reorders the rest |
| an env proxy with credentials, `http://` target | nothing | `proxy-authorization` (reqwest's source) |

Rules added:

| # | Behaviour | Who decided |
|---|---|---|
| R1a | The URL's userinfo gives a client `Authorization: Basic base64(user:password)` row, percent-decoded, listed first, as the wire has it. It follows reqwest's `extract_authority`: none when the username isn't UTF-8 once decoded, and a password that isn't is left off. None while a token still unresolved can change it (R4's parser test, asked of the unresolved names); a resolved value with braces in it is a credential like any other (review round 1). A sent request or default row naming `authorization` replaces it, as with `Accept` (R3). | task (`you`); the decoding is reqwest's, pinned by `sent_headers_match_the_wire` |
| R1b | ~~A typed `Authorization` that replaces the URL's is listed where it is typed.~~ *Replaced after review round 2:* the sent section follows the wire's own order rule, probed on a raw server: reqwest's map holds all of a name's values together, at the place the name first went in, and the URL's `Authorization` goes in first. So a row that replaces it goes out, and is listed, first; and `X-A`, `X-B`, `x-a` go out as `x-a, x-a, x-b` (this was already misordered on main). `PreparedRequest.wire` holds that list, built once by `prepare` (`wire_order`); it replaces `client_headers`. The editor keeps table order. | `mine (approved)` |
| R1c | When a secret can change it (the parser test of R4), the row shows as `Basic ●●●●●●`; otherwise as sent, since the URL bar already shows the userinfo. | `mine (approved)` |
| R3c | The client's `Content-Length` is listed only for a body with bytes in it. | follows the wire |
| R3d | `Transfer-Encoding` joins R3a's client-owned names. A typed one is listed but never sent, with `not sent · only the client may set Transfer-Encoding`, since the client never sends one for a body of known length. A chunked body can no longer be sent on purpose, and curl export leaves it out, as R9 does for `Host`/`Content-Length`. | `mine (approved)` |
| R10 | `Proxy-Authorization` is left out: it goes to the proxy, not the server, and listing it means copying reqwest's proxy env matching into core. Nothing filed. | `mine (approved)` |

Design: `url_headers` reads the `Host` and the userinfo's
`Authorization` (`userinfo_auth`, a copy of reqwest's decode) from one
parse. The R4 parser test becomes `with_stand_ins`, which gives both
headers with the named values as `1` and as `2`; `rows()` asks it once
for the secrets (mask) and once for the unresolved names (no
`Authorization` yet). `prepare` builds `PreparedRequest.wire` by R1b's
rule. The `Basic` encoding is one helper,
shared with curl import's `-u`. Core gains the `percent-encoding` crate,
already in the lock file as `url`'s.

*Review rounds 1–2:* the note `not sent · set by the client only` (R3b,
since #21) also covered rows where the client sends no such header at
all (a typed `Transfer-Encoding`; `Content-Length` on a GET or an empty
body), and read as if it did. It becomes `not sent · only the client may
set X`, which says who may set it and nothing about what goes out
(`mine (approved)`). `the client sets X` stays for when it does.

## 6. A note for #20

#20 says `Host` and `Content-Length` "cannot be overridden". Until now
the wire let a request row override them. R3a makes #20's premise true:
those two get no promote button and no strike-through, and #20 needs
nothing more for them.
