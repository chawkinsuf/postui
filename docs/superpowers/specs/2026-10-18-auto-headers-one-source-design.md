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
| R4 | `Host` is the URL's authority the way the client sends it: host lowercased, default port dropped, no userinfo. A masked host is taken verbatim. | stage7 §6; the derivation moves from `http.rs` |
| R5 | The auto section draws every non-request row that is sent, plus every unsent **default** row (struck through). Unsent auto and client rows stay hidden, as `Accept` and `Content-Type` are today. #20 will change this to strike them through. | today's rule kept |
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
- Unsent auto and client rows are in the list but hidden by the editor
  (R5), so #20's change only touches the draw.
- `HeaderOrigin::Request { row }` carries the table index for R3b,
  rather than the editor matching rows by name or by order.
- R3b's note wording: `not sent · the client sets Host`.

## 6. A note for #20

#20 says `Host` and `Content-Length` "cannot be overridden". Until now
the wire let a request row override them. R3a makes #20's premise true:
those two get no promote button and no strike-through, and #20 needs
nothing more for them.
