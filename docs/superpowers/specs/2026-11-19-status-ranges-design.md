# Status checks take ranges — Design

Task #324. Status: **waiting for approval** (questions 1 and 2 on the
ticket).

## 1. The problem

A request's status check takes exact codes only: `status = 200` or
`status = [200, 201]` in the file, `200, 201` in the request's Checks tab
(`Checks::parse_status`, the `status_codes` read in
`crates/postui-core/src/model.rs`). The user, on #319: "i have to list each
stats exactly, i cant say something like < 400 … maybe this isnt necessary
for this version, but i sort of expect it" (you). #319 made it its own task
(2: A, mine (approved)).

`[checks]` shipped in 0.5.0 (2026-11-18), so files with `status = 200` and
`status = [200, 201]` exist. They must read and mean the same, and the
app must keep writing them the same, so that 0.5.0 still reads a file
that names exact codes only.

Every path that judges or shows the status check goes through one place:
`postui_core::checks::Outcome::Status { expected, got }` (`passed`,
`failure`, `summary`). `run`, `suite` (plain, live, JSON) and the app's
response Checks tab all read those. The writers are `Checks::parse_status`
(the Checks tab's status box), `status_codes` (the file read) and
`HttpRequest::to_toml_string` (the file write).

## 2. Rules

| # | Rule | Who |
|---|---|---|
| S1 | A status check is a list of terms. A response passes it when its status matches any term. | mine |
| S2 | A term is one of: an exact code `404`; a range `200-299` (both ends included); a comparison `< 400`, `<= 399`, `> 499`, `>= 500`. | Q2 (recommended A); the ticket's examples `< 400`, `>= 500`, `200-299` |
| S3 | Every number in a term is a code from 100 to 599, as an exact code is today. A range runs low to high (`200-200` is allowed). A term that matches no code from 100 to 599 (`< 100`, `> 599`) is refused. | mine |
| S4 | In the box, terms are separated by commas or spaces, as codes are today (`200 201` still works). Spaces after a comparison sign and around a range's `-` belong to the term: `< 400`, `<400`, `200 - 299` and `200-299` all read. | mine |
| S5 | The box shows the terms back in one form: `200, 200-299, < 400` (a comparison with one space after its sign, a range with none), in the order typed. A term typed twice counts once, as a code does today. Two terms that overlap (`200, 200-299`) both stay. | mine |
| S6 | In the file, an exact code stays a number and a range or a comparison is a string: `status = ["200-299", 404]`, `status = "< 400"`. A file of exact codes reads and writes exactly as in 0.5.0. | Q1 (recommended A) |
| S7 | A string in the file reads as the box reads its text, so `"200"` reads as the code 200 and `"< 400"` as the comparison. The app writes each term back in S5's form, a code as a number. Any other value (a float, a table, a bad term) refuses the read, as a bad code does today (R6). | mine |
| S8 | 0.5.0 and older refuse a request file whose `status` holds a string: the CHANGELOG says so. A file with exact codes only stays readable by 0.5.0. | mine (the cost of any choice for Q1) |
| S9 | A failed status check says `status 500, expected 200-299 or 404`, the terms in S5's form joined by `or`, as codes are joined today. A passed one says `status 204`, as today. | mine |
| S10 | The `run --json` / `suite --json` check is `{"check": "status", "expected": [200, "200-299", "< 400"], "got": 500, …}`: each term as the file writes it, a code as a number and the rest as strings. A check of exact codes is the JSON 0.5.0 gave. | Q1 (follows the file) |
| S11 | The box refuses a bad term with a warning toast and keeps what it had, as today: `a check's status must be codes from 100 to 599, ranges like 200-299 or comparisons like < 400, not 600`; a range the wrong way round: `a check's status range must run low to high, not 300-200`; a term that matches nothing: `a check's status must match a code from 100 to 599, not < 100`. The file read gives the same words. | mine |
| S12 | The status box's footer text becomes `Codes that pass, e.g. 200, 201-204, < 400`. Its placeholders (`any code below 400`, `run and suite: any code below 400`) stay. | mine |
| S13 | An empty status is still no status check (below 400 in `run` and `suite`; checks tabs spec H1). `< 400` typed in the box is a status check: it judges the response in the app even with no other check (checks tabs spec P1), and its line reads `status 200`. | mine |
| S14 | A status a server sends outside 100 to 599 (an HTTP client takes up to 999) is judged by the same comparisons: `>= 500` passes a 600. | mine |

Not included (each could be added later without changing any file
written by this design): the class form `2xx`; `!=` / not-in; ranges
open at one end (`500-`).

## 3. Design

**Core model** (`postui-core/src/model.rs`). `Checks::status` changes from
`Vec<u16>` to `Vec<StatusTerm>`:

```rust
pub enum StatusTerm {
    Code(u16),
    Range(u16, u16),        // both ends included
    Below(u16), AtMost(u16),     // < n, <= n
    Above(u16), AtLeast(u16),    // > n, >= n
}
```

with `matches(code) -> bool`, `Display` (S5's form), and a parse of one
term's text. `Checks::parse_status(text) -> Result<Vec<StatusTerm>,
String>` reads the box (S4) and is what the file read calls on a string
(S7). The serde read (`status_codes`) takes a number, a string, or a list
of both. The write (`to_toml_string`) writes one term bare and several as
a list, a `Code` as an integer and any other term as its `Display` string.

This is a contract change: `Checks::status`'s type is matched or built in
`checks.rs`, `cli/checks.rs`, `components/editor.rs`, `http.rs` and tests.

**Core checks** (`postui-core/src/checks.rs`).
`Outcome::Status { expected: Vec<StatusTerm>, got }`. `passed` is
`expected.iter().any(|t| t.matches(got))`; `failure` joins the terms'
`Display` with ` or ` (S9).

**CLI** (`cli/checks.rs`). `Kind::Status::expected` serializes each term
as S10 says (a `Serialize` on `StatusTerm`: a number for `Code`, the
`Display` string otherwise).

**App** (`components/editor.rs`). `Editor::check_status:
Vec<StatusTerm>`; `check_field_text(0)` joins the terms' `Display` with
`, `; `CHECK_FIELDS[0].hint` changes (S12). No other app code changes:
the response's Checks tab already shows `Outcome::summary`.

**Docs.** `docs/guide.md` (Checks tab and Command line), `skills/postui`
(`reference.md`'s `[checks]` paragraph, SKILL.md if it names the status
form), the CHANGELOG's Unreleased (Added, and the 0.5.0 compatibility
line S8), the suite checks spec §2.1/§2.2 and the checks tabs spec's H8
get a dated pointer here.

Nothing is removed.

## 4. Tests

- Core model: each term form parses from the box (S2, S4: with and without
  spaces), shows back in S5's form, refuses each bad form with S11's
  words; a mixed list round-trips through the file (S6, S7), a file of
  exact codes writes byte-for-byte as before; a string code reads; a bad
  string refuses the read; `"< 100"` refuses.
- Core checks: each comparison and a range pass and fail at their edges
  (399/400 for `< 400`, 199/200/299/300 for `200-299`); a mixed list
  passes on any term; the failure line (S9).
- CLI: `run --json` gives the mixed `expected` (S10); a file of exact codes
  gives 0.5.0's JSON (the existing tests stay).
- App: typing `200-299, <400` in the status box commits, shows
  `200-299, < 400`, saves `status = ["200-299", "< 400"]`; a bad term
  toasts S11's words and keeps the old text.
- TUI check in tmux: type a range in the box, send, read the response's
  Checks tab and the saved file.

## 5. `mine` choices for veto

S3 (100–599 for every number, so `< 600` is refused and `<= 599` is the
way to say "any"), S4 (spaces inside a term), S5 (one display form;
overlaps kept), S7 (a string may hold an exact code, read leniently),
S9/S11/S12 wording, S13, S14.
