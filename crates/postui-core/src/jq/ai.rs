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
    let current = if current_filter.trim().is_empty() {
        "(none)"
    } else {
        current_filter.trim()
    };
    let example = match &shape.example_path {
        Some(path) => format!("\nA full path in this document: {path}\n"),
        None => String::new(),
    };
    format!(
        "{SYSTEM}\n\nStructure:\n{}\n{example}\nCurrent filter: {current}\n\nRequest: {request}\n",
        shape.outline
    )
}

/// Trims, skips fence lines, and takes the first line that has something
/// left once its backticks are unwrapped (`Line::filter`): a line that is
/// only backticks falls through to the next.
pub fn extract_filter(reply: &str) -> Option<String> {
    reply
        .lines()
        .map(str::trim)
        .map(Line::parse)
        .filter(|line| !line.is_fence())
        .map(|line| line.filter())
        .find(|l| !l.is_empty())
}

/// One reply line, cut at its backticks the way Markdown reads them. A run
/// of N backticks opens a span that the next run of exactly N closes (or
/// the end of the line does), so `` `` `.a` `` `` quotes `` `.a` ``.
/// Backticks are not jq syntax, but they are legal inside a string literal
/// (`test("`")`): a backtick inside quotes is text — inside a span always,
/// and outside one when the line's quotes balance (a bare filter); a line
/// with a stray `"` is prose, where a quote is just a character. (A `"`
/// inside a `\(…)` interpolation counts like any other; that nesting is
/// not tracked.)
struct Line<'a> {
    raw: &'a str,
    /// Text before the first span.
    head: &'a str,
    /// Whether a run closed the last span (else the line's end did).
    last_closed: bool,
    /// Each span with the length of the run that opened it.
    spans: Vec<(usize, &'a str)>,
    /// Text between consecutive spans: `spans.len() - 1` of them.
    gaps: Vec<&'a str>,
    /// Text after the last span (empty when the last span is unclosed).
    tail: &'a str,
}

impl<'a> Line<'a> {
    fn parse(line: &'a str) -> Self {
        let mut out = Line {
            raw: line,
            head: "",
            last_closed: false,
            spans: Vec::new(),
            gaps: Vec::new(),
            tail: "",
        };
        let balanced = quotes_balanced(line);
        let mut pos = 0;
        loop {
            let Some((start, open)) = next_run(line, pos, balanced) else {
                let text = &line[pos..];
                if out.spans.is_empty() {
                    out.head = text;
                } else {
                    out.tail = text;
                }
                return out;
            };
            let text = &line[pos..start];
            if out.spans.is_empty() {
                out.head = text;
            } else {
                out.gaps.push(text);
            }
            let content_from = start + open;
            // A span whose quotes do not balance (`` `the "id` field ``) is
            // prose: its closer is found with quotes read as characters.
            let close = closing_run(line, content_from, open, true)
                .or_else(|| closing_run(line, content_from, open, false));
            let content_to = close.unwrap_or(line.len());
            out.spans.push((open, &line[content_from..content_to]));
            out.last_closed = close.is_some();
            match close {
                Some(at) => pos = at + open,
                None => return out,
            }
        }
    }

    /// A line that is a code fence: it starts with three or more backticks
    /// that nothing closes, followed by at most an info string (```` ``` ````,
    /// ```` ```jq ````, ```` ``` jq filter ````). Anything that is not plain
    /// words after the run (```` ```jq .a ````) is a filter the model forgot
    /// to put on its own line.
    fn is_fence(&self) -> bool {
        self.head.is_empty()
            && !self.last_closed
            && matches!(
                self.spans.as_slice(),
                [(n, content)] if *n >= 3
                    && content.chars().all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
            )
    }

    /// The filter this line carries. No backticks: the line itself. One
    /// quoted span: its text (`` `.a` and more `` → `.a`, so prose around
    /// a quoted filter is dropped; a one-line ```` ```jq .a``` ```` loses the
    /// language word). Several spans and nothing but jq operators between
    /// them: one filter quoted piecewise, put back together. Several spans
    /// with words anywhere: prose, and the first non-empty span wins.
    /// Nothing inside any span (`.a``, ``` `` ```): the line with its
    /// backticks dropped.
    fn filter(&self) -> String {
        if self.spans.is_empty() {
            return self.head.to_string();
        }
        let texts: Vec<String> = self
            .spans
            .iter()
            .map(|(run, content)| {
                let content = Self::without_backticks(content);
                let content = content.trim();
                match content.strip_prefix("jq ") {
                    Some(rest) if *run >= 3 => rest.trim().to_string(),
                    _ => content.to_string(),
                }
            })
            .collect();
        let piecewise = texts.len() > 1
            && self.head.trim().is_empty()
            && self.tail.trim().is_empty()
            && texts.iter().all(|t| !t.is_empty())
            && self.gaps.iter().all(|gap| is_operator(gap.trim()));
        if piecewise {
            let mut out = texts[0].clone();
            for (gap, text) in self.gaps.iter().zip(&texts[1..]) {
                out.push(' ');
                out.push_str(gap.trim());
                out.push(' ');
                out.push_str(text);
            }
            return out;
        }
        texts
            .into_iter()
            .find(|t| !t.is_empty())
            .unwrap_or_else(|| Self::without_backticks(self.raw).trim().to_string())
    }

    /// `s` with every backtick outside a string literal removed.
    fn without_backticks(s: &str) -> String {
        let mut out = String::new();
        let mut pos = 0;
        while let Some((at, len)) = next_run(s, pos, true) {
            out.push_str(&s[pos..at]);
            pos = at + len;
        }
        out.push_str(&s[pos..]);
        out
    }
}

/// Whether every unescaped `"` in `s` has its partner.
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
/// literal (an odd number of unescaped `"` since `from`) is text.
fn next_run(s: &str, from: usize, in_filter: bool) -> Option<(usize, usize)> {
    let mut in_literal = false;
    let mut chars = s[from..].char_indices().map(|(i, c)| (from + i, c));
    while let Some((i, c)) = chars.next() {
        match c {
            '`' if !in_literal => {
                let len = s[i..].chars().take_while(|&c| c == '`').count();
                return Some((i, len));
            }
            '\\' if in_literal => {
                chars.next();
            }
            '"' if in_filter => in_literal = !in_literal,
            _ => {}
        }
    }
    None
}

/// A jq binary operator on its own: what may sit between two quoted
/// pieces of one filter. Not `,`: two filters either side of a comma are
/// as likely two alternatives as one expression.
fn is_operator(gap: &str) -> bool {
    matches!(
        gap,
        "|" | "//" | "+" | "-" | "*" | "/" | "%" | "<" | ">" | "<=" | ">=" | "==" | "!=" | "and"
            | "or"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(outline: &str, example: Option<&str>) -> Shape {
        Shape {
            outline: outline.into(),
            example_path: example.map(str::to_string),
        }
    }

    #[test]
    fn the_prompt_carries_the_structure_the_example_the_filter_and_the_request_in_order() {
        let p = prompt(&shape("a\n  b: number", Some(".a.b")), ".a", "double it");
        let shape_at = p
            .find("Structure:\na\n  b: number\n")
            .expect("structure block");
        let example_at = p
            .find("A full path in this document: .a.b")
            .expect("example line");
        let filter_at = p.find("Current filter: .a").expect("filter line");
        let req_at = p.find("Request: double it").expect("request line");
        assert!(
            shape_at < example_at && example_at < filter_at && filter_at < req_at,
            "{p}"
        );
        assert!(
            p.contains("jq 1.7"),
            "system section names the dialect: {p}"
        );
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
        assert_eq!(
            extract_filter("  .a | length \n"),
            Some(".a | length".into())
        );
        assert_eq!(
            extract_filter("```jq\n.a | length\n```"),
            Some(".a | length".into())
        );
        assert_eq!(
            extract_filter("```\n\n.a\n\nexplanation\n```"),
            Some(".a".into())
        );
        assert_eq!(extract_filter("\n\n"), None);
    }

    #[test]
    fn backticks_around_the_reply_are_dropped() {
        assert_eq!(
            extract_filter("`.data[] | .name`\n"),
            Some(".data[] | .name".into())
        );
        assert_eq!(extract_filter("`` .a ``"), Some(".a".into()));
        assert_eq!(extract_filter("``"), None);
        assert_eq!(extract_filter(".a`"), Some(".a".into()));
    }

    #[test]
    fn prose_around_a_backticked_filter_is_dropped() {
        assert_eq!(
            extract_filter("`.data[] | .name` returns the names"),
            Some(".data[] | .name".into())
        );
        assert_eq!(
            extract_filter("Use `map(.id)` here."),
            Some("map(.id)".into())
        );
        assert_eq!(extract_filter("`.a"), Some(".a".into()), "unclosed");
    }

    #[test]
    fn a_backtick_inside_a_string_literal_is_part_of_the_filter() {
        assert_eq!(
            extract_filter(".items[] | select(.code | test(\"`\"))"),
            Some(".items[] | select(.code | test(\"`\"))".into())
        );
        assert_eq!(
            extract_filter(".name == \"a`b\""),
            Some(".name == \"a`b\"".into())
        );
        assert_eq!(
            extract_filter(".x == \"say \\\"hi\\\"\" | `.y`"),
            Some(".y".into()),
            "escaped quotes do not open a literal"
        );
        assert_eq!(
            extract_filter("`.items[] | select(.code | test(\"`\"))` matches"),
            Some(".items[] | select(.code | test(\"`\"))".into()),
            "the closing backtick is outside the literal too"
        );
    }

    #[test]
    fn a_filter_quoted_piecewise_is_joined_and_prose_between_spans_is_not() {
        assert_eq!(
            extract_filter("`.data[] | select(.active)` | `.name`"),
            Some(".data[] | select(.active) | .name".into())
        );
        assert_eq!(
            extract_filter("`.a` and `.b` are both fine"),
            Some(".a".into()),
            "words between spans: prose, first span wins"
        );
        assert_eq!(extract_filter("`.x` // `.y`"), Some(".x // .y".into()));
        assert_eq!(
            extract_filter("`.items | length` > `0`"),
            Some(".items | length > 0".into())
        );
        assert_eq!(
            extract_filter("`.a` and `.b`"),
            Some(".a and .b".into()),
            "a word operator with nothing else between the spans joins"
        );
        assert_eq!(
            extract_filter("Either `.a`, `.b`"),
            Some(".a".into()),
            "a comma lists alternatives"
        );
        assert_eq!(
            extract_filter("Use `.a` | `.b`"),
            Some(".a".into()),
            "words before the spans are prose like words between them"
        );
    }

    #[test]
    fn a_one_line_triple_backtick_reply_is_unwrapped_but_fence_lines_are_skipped() {
        assert_eq!(
            extract_filter("```.data[] | .name```"),
            Some(".data[] | .name".into())
        );
        assert_eq!(extract_filter("```jq .a```"), Some(".a".into()));
        assert_eq!(
            extract_filter("```jq .data[] | .name\n```"),
            Some(".data[] | .name".into()),
            "a filter on the opener's line is not an info string"
        );
        assert_eq!(extract_filter("```.a"), Some(".a".into()));
        assert_eq!(extract_filter("```jq\n.a\n```"), Some(".a".into()));
        assert_eq!(extract_filter("``` jq\n.data[].name\n```"), Some(".data[].name".into()));
        assert_eq!(extract_filter("```jq filter\n.a\n```"), Some(".a".into()));
        assert_eq!(extract_filter("```\n```"), None);
    }

    #[test]
    fn a_double_backtick_span_quotes_single_backticks_which_are_then_dropped() {
        assert_eq!(extract_filter("`` `.a` ``"), Some(".a".into()));
        assert_eq!(extract_filter("`` .a ``"), Some(".a".into()));
    }

    #[test]
    fn span_edge_cases_hold() {
        assert_eq!(extract_filter("Try `.a"), Some(".a".into()), "unclosed after prose");
        assert_eq!(extract_filter("`.a`."), Some(".a".into()), "punctuation after");
        assert_eq!(extract_filter("`.a` `.b`"), Some(".a".into()), "no operator between");
        assert_eq!(
            extract_filter("`select(.x == \"a`b\")`"),
            Some("select(.x == \"a`b\")".into()),
            "a backtick inside the literal stays"
        );
        assert_eq!(extract_filter("` `"), None, "a blank span is nothing");
        assert_eq!(extract_filter("```"), None);
        assert_eq!(
            extract_filter("Use the \"name field: `.data[].name`"),
            Some(".data[].name".into()),
            "a stray quote in prose is not a string literal"
        );
        assert_eq!(
            extract_filter("`select(.x == \"a)` | `.b`"),
            Some("select(.x == \"a) | .b".into()),
            "a stray quote inside a span does not swallow its closer"
        );
        assert_eq!(
            extract_filter("Use `.a` (the \"id` field)"),
            Some(".a".into())
        );
    }

    #[test]
    fn a_line_of_only_backticks_falls_through_to_the_next() {
        assert_eq!(
            extract_filter("``\n.data[] | .name\n"),
            Some(".data[] | .name".into())
        );
        assert_eq!(extract_filter("``\n\n"), None);
    }
}
