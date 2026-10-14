//! The text handed to the configured AI command, and the reply cleanup.

use super::shape::Shape;

const SYSTEM: &str = "\
You write jq 1.7 filters for a JSON document whose structure is given below.
Reply with exactly one jq filter on a single line and nothing else: no prose,
no code fences, no backticks. Prefer `map(select(...))` over
`.[] | select(...)` so results stay arrays. Prefer `sort_by`, `group_by`,
`unique_by`, `to_entries`, `length`, `keys` over hand-rolled reductions. If a
current filter is given and the request refines it, extend that filter with
` | `; the structure describes the document before any filter. If the request
is ambiguous, pick the most literal reading.
The structure lists every key in the document once, mostly one per line; a
line indented under another is a key inside it. `name[]` is an array, and the
lines under it are the keys of its elements, merged across all elements.
`key?` means some of the objects lack that key. A key with several types
(`a: object | array`) lists its object's keys under it plus a `[]` line for its
array's elements, reached with `.a[]`. `name: object, N keys (k1, k2, … M more)`
is an object with many keys whose values all share one structure: its keys are
named on that line (`… M more` when they are too many to list; `k?` when some
of the objects lack it), and the lines under its `[]` describe every value,
merged. Reach one value with `.name.k1` (`.k1` when the folded object is the
top level, `.`), all of them with `.name[]`, the keys with `keys` or
`to_entries`. `… N more keys` ends a key list too long to show in full, and
`… N more lines not shown` stands for lines cut from the structure at that
point and below it.
Values are shown as their type name; `a | b` means both types occur.
`{} (always empty)` and `[] (always empty)`, alone or in such a list, are
containers that never held anything. Keys that are not plain identifiers are
quoted. The request may
name only the last key of a path: find that key in the structure and
write its whole path from the root, through every array with `[]`. Never
write a bare `.key` unless that key is at the top level.";

/// The whole stdin text for the AI command: system section, structure,
/// example path, current filter, and the user's sentence.
pub fn prompt(shape: &Shape, current_filter: &str, request: &str) -> String {
    let current = if current_filter.trim().is_empty() { "(none)" } else { current_filter.trim() };
    let example = match &shape.example_path {
        Some(path) => format!("\nA full path in this document: {path}\n"),
        None => String::new(),
    };
    format!(
        "{SYSTEM}\n\nStructure:\n{}\n{example}\nCurrent filter: {current}\n\nRequest: {request}\n",
        shape.outline
    )
}

/// The filter in a reply. Lines are trimmed and fence lines skipped; then
/// the first line that opens with a backtick wins (prose often sits on its
/// own line above a quoted filter), else the first non-empty line. What a
/// line yields is `Line::filter`.
pub fn extract_filter(reply: &str) -> Option<String> {
    let lines: Vec<Line> =
        reply.lines().map(str::trim).map(Line::parse).filter(|line| !line.is_fence()).collect();
    let quoted = lines.iter().filter(|line| line.is_quoted());
    quoted.chain(lines.iter()).map(Line::filter).find(|filter| !filter.is_empty())
}

/// One reply line, cut at its backticks the way Markdown reads them: a run
/// of N backticks opens a span that the next run of exactly N closes (or
/// the end of the line does), so `` `` `.a` `` `` quotes `` `.a` ``.
/// Backticks are not jq syntax, but they are legal inside a string literal
/// (`test("`")`): when the line's quotes balance, a backtick inside quotes
/// is text; a line with a stray `"` is prose, where a quote is just a
/// character. (A `"` inside a `\(…)` interpolation counts like any other;
/// that nesting is not tracked.)
struct Line<'a> {
    raw: &'a str,
    /// Whether `"` opens a string literal on this line.
    in_filter: bool,
    /// Each span with the length of the run that opened it.
    spans: Vec<(usize, &'a str)>,
    /// Whether a run closed the last span (else the line's end did).
    last_closed: bool,
}

impl<'a> Line<'a> {
    fn parse(line: &'a str) -> Self {
        let in_filter = quotes_balanced(line);
        let mut out = Line { raw: line, in_filter, spans: Vec::new(), last_closed: false };
        let mut pos = 0;
        while let Some((start, open)) = next_run(line, pos, in_filter) {
            let from = start + open;
            let close = closing_run(line, from, open, in_filter);
            out.spans.push((open, &line[from..close.unwrap_or(line.len())]));
            out.last_closed = close.is_some();
            match close {
                Some(at) => pos = at + open,
                None => break,
            }
        }
        out
    }

    /// A line that opens with a backtick: it quotes its filter.
    fn is_quoted(&self) -> bool {
        self.raw.starts_with('`') && !self.spans.is_empty()
    }

    /// A line that is a code fence: it starts with three or more backticks
    /// that nothing closes, followed by at most a language word
    /// (```` ``` ````, ```` ```jq ````, ```` ``` jq ````). Anything more
    /// after the run (```` ```jq .a ````, ```` ```jq length ````) is a filter
    /// the model forgot to put on its own line.
    fn is_fence(&self) -> bool {
        self.raw.starts_with('`')
            && !self.last_closed
            && matches!(
                self.spans.as_slice(),
                [(n, content)] if *n >= 3
                    && content.trim().chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
            )
    }

    /// The filter this line carries. A line that opens with a backtick is
    /// quoting it: the first span with text in it (`` `.a` and more `` →
    /// `.a`, so what follows the quote is dropped, and so is a leading `jq `
    /// language word; `.a``: the line with its backticks dropped). Any other
    /// line stands as it is — a backtick later in a sentence is prose, and
    /// a sentence lands for repair rather than a fragment of it landing as
    /// a filter.
    fn filter(&self) -> String {
        if !self.is_quoted() {
            return self.raw.to_string();
        }
        self.spans
            .iter()
            .map(|(_, content)| {
                let text = without_backticks(content, self.in_filter);
                let text = text.trim();
                text.strip_prefix("jq ").map_or(text, str::trim).to_string()
            })
            .find(|text| !text.is_empty())
            .unwrap_or_else(|| without_backticks(self.raw, self.in_filter).trim().to_string())
    }
}

/// `s` with every run of backticks (`next_run`) removed.
fn without_backticks(s: &str, in_filter: bool) -> String {
    let mut out = String::new();
    let mut pos = 0;
    while let Some((at, len)) = next_run(s, pos, in_filter) {
        out.push_str(&s[pos..at]);
        pos = at + len;
    }
    out.push_str(&s[pos..]);
    out
}

/// Whether every `"` in `s` has its partner; `\"` is neither, as in
/// `next_run`.
fn quotes_balanced(s: &str) -> bool {
    let mut open = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => open = !open,
            _ => {}
        }
    }
    !open
}

/// Where the run of exactly `open` backticks that closes a span starting
/// at `from` begins, if any.
fn closing_run(line: &str, from: usize, open: usize, in_filter: bool) -> Option<usize> {
    let mut from = from;
    loop {
        match next_run(line, from, in_filter) {
            Some((at, len)) if len == open => return Some(at),
            Some((at, len)) => from = at + len,
            None => return None,
        }
    }
}

/// The next run of backticks in `s` at or after `from`: where it starts
/// and how long it is. With `in_filter`, a backtick inside a jq string
/// literal (an odd number of `"` since `from`, `\"` not counted) is text.
fn next_run(s: &str, from: usize, in_filter: bool) -> Option<(usize, usize)> {
    let mut in_literal = false;
    let mut chars = s[from..].char_indices().map(|(i, c)| (from + i, c));
    while let Some((i, c)) = chars.next() {
        match c {
            '`' if !in_literal => {
                let len = s[i..].chars().take_while(|&c| c == '`').count();
                return Some((i, len));
            }
            '\\' => {
                chars.next();
            }
            '"' if in_filter => in_literal = !in_literal,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(outline: &str, example: Option<&str>) -> Shape {
        Shape { outline: outline.into(), example_path: example.map(str::to_string) }
    }

    #[test]
    fn the_prompt_carries_the_structure_the_example_the_filter_and_the_request_in_order() {
        let p = prompt(&shape("a\n  b: number", Some(".a.b")), ".a", "double it");
        let shape_at = p.find("Structure:\na\n  b: number\n").expect("structure block");
        let example_at = p.find("A full path in this document: .a.b").expect("example line");
        let filter_at = p.find("Current filter: .a").expect("filter line");
        let req_at = p.find("Request: double it").expect("request line");
        assert!(shape_at < example_at && example_at < filter_at && filter_at < req_at, "{p}");
        assert!(p.contains("jq 1.7"), "system section names the dialect: {p}");
        assert!(p.contains("no backticks"), "{p}");
        assert!(
            p.contains("write its whole path from the root"),
            "a bare leaf name must be turned into a path: {p}"
        );
        assert!(
            p.contains("`name: object, N keys (k1, k2, … M more)`"),
            "the folded object line is explained: {p}"
        );
        assert!(
            p.contains("a `[]` line for its\narray's elements, reached with `.a[]`"),
            "the `[]` line under a key of several types is explained: {p}"
        );
        assert!(
            p.contains("`{} (always empty)` and `[] (always empty)`, alone or in such a list"),
            "the always-empty containers are explained: {p}"
        );
    }

    #[test]
    fn an_empty_current_filter_is_spelled_out_and_a_missing_example_is_left_out() {
        let p = prompt(&shape(".: number", None), "", "x");
        assert!(p.contains("Current filter: (none)"), "{p}");
        assert!(!p.contains("A full path"), "{p}");
    }

    #[test]
    fn replies_are_trimmed_unfenced_and_cut_to_their_first_line() {
        assert_eq!(extract_filter("  .a | length \n"), Some(".a | length".into()));
        assert_eq!(extract_filter("```jq\n.a | length\n```"), Some(".a | length".into()));
        assert_eq!(extract_filter("```\n\n.a\n\nexplanation\n```"), Some(".a".into()));
        assert_eq!(extract_filter("\n\n"), None);
    }

    #[test]
    fn backticks_around_the_reply_are_dropped() {
        assert_eq!(extract_filter("`.data[] | .name`\n"), Some(".data[] | .name".into()));
        assert_eq!(extract_filter("`` .a ``"), Some(".a".into()));
        assert_eq!(extract_filter("``"), None);
        assert_eq!(
            extract_filter(".a`"),
            Some(".a`".into()),
            "a stray backtick at the end is not a quote; the line lands for repair"
        );
    }

    #[test]
    fn prose_after_a_quoted_filter_is_dropped_but_a_sentence_stands() {
        assert_eq!(
            extract_filter("`.data[] | .name` returns the names"),
            Some(".data[] | .name".into())
        );
        assert_eq!(
            extract_filter("Use `map(.id)` here."),
            Some("Use `map(.id)` here.".into()),
            "a backtick mid-sentence is prose; the sentence lands for repair"
        );
        assert_eq!(extract_filter("`.a"), Some(".a".into()), "unclosed");
    }

    #[test]
    fn a_backtick_inside_a_string_literal_is_part_of_the_filter() {
        assert_eq!(
            extract_filter(".items[] | select(.code | test(\"`\"))"),
            Some(".items[] | select(.code | test(\"`\"))".into())
        );
        assert_eq!(extract_filter(".name == \"a`b\""), Some(".name == \"a`b\"".into()));
        assert_eq!(
            extract_filter(".x == \"say \\\"hi\\\"\" | `.y`"),
            Some(".x == \"say \\\"hi\\\"\" | `.y`".into()),
            "a line that does not open with a backtick stands as it is"
        );
        assert_eq!(
            extract_filter("`.items[] | select(.code | test(\"`\"))` matches"),
            Some(".items[] | select(.code | test(\"`\"))".into()),
            "the closing backtick is outside the literal too"
        );
    }

    #[test]
    fn among_several_spans_the_first_wins_and_a_sentence_is_never_cut_to_one() {
        // The prompt asks for one filter and no backticks; two quoted
        // pieces are read as alternatives, never glued into one filter, and
        // a sentence that quotes pieces lands whole.
        for (reply, filter) in [
            ("`.data[] | select(.active)` | `.name`", ".data[] | select(.active)"),
            ("`.a` and `.b` are both fine", ".a"),
            ("`.x` // `.y`", ".x"),
            ("`.a` or `.b`", ".a"),
            ("Either `.a`, `.b`", "Either `.a`, `.b`"),
            ("Use `.a` | `.b`", "Use `.a` | `.b`"),
        ] {
            assert_eq!(extract_filter(reply), Some(filter.into()), "{reply}");
        }
    }

    #[test]
    fn a_quoted_line_beats_prose_but_a_mention_inside_prose_does_not() {
        assert_eq!(
            extract_filter("Here is the filter:\n`.data[] | .name`"),
            Some(".data[] | .name".into())
        );
        assert_eq!(
            extract_filter("Here is the filter:\n.data[] | .name"),
            Some("Here is the filter:".into()),
            "with no quoted line, the first line stands (lands for hand repair)"
        );
        assert_eq!(
            extract_filter("Starting from `.data[]`, the filter is:\n.data[] | .name"),
            Some("Starting from `.data[]`, the filter is:".into()),
            "a mention mid-sentence must not land as a wrong-but-valid filter"
        );
    }

    #[test]
    fn quotes_in_a_quoted_line_are_read_as_a_filter_reads_them() {
        assert_eq!(
            extract_filter("`.a | test(\"\\\\\\\"\")` matches a quote"),
            Some(".a | test(\"\\\\\\\"\")".into()),
            "an escaped quote inside the literal"
        );
        assert_eq!(
            extract_filter("`.name == \"x` is wrong; use `.name == \"x\"`"),
            Some(".name == \"x".into()),
            "unbalanced quotes: spans close at their backticks, first span wins"
        );
    }

    #[test]
    fn a_one_line_triple_backtick_reply_is_unwrapped_but_fence_lines_are_skipped() {
        assert_eq!(extract_filter("```.data[] | .name```"), Some(".data[] | .name".into()));
        assert_eq!(extract_filter("```jq .a```"), Some(".a".into()));
        assert_eq!(
            extract_filter("```jq .data[] | .name\n```"),
            Some(".data[] | .name".into()),
            "a filter on the opener's line is not an info string"
        );
        assert_eq!(extract_filter("```.a"), Some(".a".into()));
        assert_eq!(
            extract_filter("```jq length\n```"),
            Some("length".into()),
            "a one-word filter after the language word is not an info string"
        );
        assert_eq!(extract_filter("```jq\n.a\n```"), Some(".a".into()));
        assert_eq!(extract_filter("``` jq\n.data[].name\n```"), Some(".data[].name".into()));
        assert_eq!(extract_filter("```\n```"), None);
    }

    #[test]
    fn a_double_backtick_span_quotes_single_backticks_which_are_then_dropped() {
        assert_eq!(extract_filter("`` `.a` ``"), Some(".a".into()));
        assert_eq!(extract_filter("`` .a ``"), Some(".a".into()));
    }

    #[test]
    fn span_edge_cases_hold() {
        assert_eq!(extract_filter("`.a`."), Some(".a".into()), "punctuation after");
        assert_eq!(extract_filter("`.a` `.b`"), Some(".a".into()), "two spans");
        assert_eq!(
            extract_filter("`select(.x == \"a`b\")`"),
            Some("select(.x == \"a`b\")".into()),
            "a backtick inside the literal stays"
        );
        assert_eq!(extract_filter("` `"), None, "a blank span is nothing");
        assert_eq!(extract_filter("```"), None);
        assert_eq!(
            extract_filter("`select(.x == \"a)` | `.b`"),
            Some("select(.x == \"a)".into()),
            "a stray quote inside a span does not swallow its closer"
        );
        assert_eq!(
            extract_filter("Try \"the simple form: `.data[].name`\" first."),
            Some("Try \"the simple form: `.data[].name`\" first.".into()),
            "prose stands whole, quotes or not"
        );
    }

    #[test]
    fn a_line_of_only_backticks_falls_through_to_the_next() {
        assert_eq!(extract_filter("``\n.data[] | .name\n"), Some(".data[] | .name".into()));
        assert_eq!(extract_filter("``\n\n"), None);
    }
}
