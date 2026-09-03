//! The text handed to the configured AI command, and the reply cleanup.

const SYSTEM: &str = "\
You write jq 1.7 filters for a JSON document whose structure is given below.
Reply with exactly one jq filter on a single line and nothing else: no prose,
no code fences. Prefer `map(select(...))` over `.[] | select(...)` so results
stay arrays. Prefer `sort_by`, `group_by`, `unique_by`, `to_entries`,
`length`, `keys` over hand-rolled reductions. If a current filter is given and
the request refines it, extend that filter with ` | `. If the request is
ambiguous, pick the most literal reading.
In the structure below, scalars are shown as their type name and arrays as a
single representative element followed by an item count.";

/// The whole stdin text for the AI command: system section, shape, current
/// filter, and the user's sentence.
pub fn prompt(shape: &str, current_filter: &str, request: &str) -> String {
    let current = if current_filter.trim().is_empty() { "(none)" } else { current_filter.trim() };
    format!("{SYSTEM}\n\nStructure: {shape}\nCurrent filter: {current}\n\nRequest: {request}\n")
}

/// Trims, strips a surrounding ``` fence, takes the first non-empty line.
pub fn extract_filter(reply: &str) -> Option<String> {
    reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("```"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_the_shape_the_current_filter_and_the_request_in_order() {
        let p = prompt(r#"{"a": number}"#, ".a", "double it");
        let shape_at = p.find(r#"Structure: {"a": number}"#).expect("shape line");
        let filter_at = p.find("Current filter: .a").expect("filter line");
        let req_at = p.find("Request: double it").expect("request line");
        assert!(shape_at < filter_at && filter_at < req_at, "{p}");
        assert!(p.contains("jq 1.7"), "system section names the dialect: {p}");
        assert!(p.contains("no code fences"), "{p}");
    }

    #[test]
    fn an_empty_current_filter_is_spelled_out() {
        assert!(prompt("{}", "", "x").contains("Current filter: (none)"));
    }

    #[test]
    fn replies_are_trimmed_unfenced_and_cut_to_their_first_line() {
        assert_eq!(extract_filter("  .a | length \n"), Some(".a | length".into()));
        assert_eq!(extract_filter("```jq\n.a | length\n```"), Some(".a | length".into()));
        assert_eq!(extract_filter("```\n\n.a\n\nexplanation\n```"), Some(".a".into()));
        assert_eq!(extract_filter("\n\n"), None);
    }
}
