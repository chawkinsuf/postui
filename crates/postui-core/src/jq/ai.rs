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
The structure lists every key in the document once, one per line; a line
indented under another is a key inside it. `name[]` is an array, and the
lines under it are the keys of its elements, merged across all elements.
`key?` means some of the objects lack that key. `name: map, N keys like \"k\"`
is an object used as a lookup table: its keys are data, and the lines under its
`[]` describe every value, merged. Reach the values with `.name[]`, one of them
with `.name[\"k\"]`, the keys with `keys` or `to_entries`. Values are shown as
their type name; `a | b` means both types occur. Keys that are not plain
identifiers are quoted. The request may name only the last key of a path: find that key in the
structure and write its whole path from the root, through every array with
`[]`. Never write a bare `.key` unless that key is at the top level.";

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

/// Trims, skips ``` fence lines, takes the first non-empty line, and drops
/// backticks wrapped around it (never jq syntax).
pub fn extract_filter(reply: &str) -> Option<String> {
    reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("```"))
        .map(|l| l.trim_matches('`').trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
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
            p.contains("`name: map, N keys like \"k\"`"),
            "the map line is explained: {p}"
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
    }
}
