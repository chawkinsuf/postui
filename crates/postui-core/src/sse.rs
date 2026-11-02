//! Server-sent events (`text/event-stream`), as streamed LLM APIs send
//! them, read as the stream of JSON values their events carry (#220).
//!
//! An SSE body is not JSON, but its events' `data:` payloads usually are,
//! one value per event. [`json_payloads`] turns such a body into the NDJSON
//! text of those payloads, so the jq engine and the Tree tab read it the
//! way they read any stream (#218): the filter runs once per event.

/// The JSON payloads of `body`'s events, one per line, when `body` is a
/// server-sent events stream; `None` when it isn't one.
///
/// A body is an event stream by its shape, whatever its content type, as
/// a stream of JSON values is (#218): every line is a field the standard
/// names (`data`, `event`, `id`, `retry`), a `:` comment or blank, and at
/// least one is `data`. Only the data reaches the result: an event's
/// `event` name and `id` are dropped, so a filter sees the payload itself
/// (Anthropic's payloads repeat their event name as `.type`). A payload
/// that isn't JSON, such as OpenAI's closing `[DONE]`, is skipped rather
/// than failing the whole body. Unlike the standard, an event the body ends
/// in without a blank line is kept: the body is a finished response, and a
/// stream cut short should still show what it got.
pub fn json_payloads(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut data: Option<String> = None;
    let mut any_data = false;
    for text in body.lines() {
        match line(text)? {
            Line::Blank => push_json(data.take(), &mut out),
            Line::Field("data", value) => {
                any_data = true;
                match &mut data {
                    // Several `data` lines in one event are one payload,
                    // joined by newlines, as the standard says.
                    Some(d) => {
                        d.push('\n');
                        d.push_str(value);
                    }
                    None => data = Some(value.to_string()),
                }
            }
            Line::Comment | Line::Field(..) => {}
        }
    }
    push_json(data, &mut out);
    any_data.then_some(out)
}

/// Whether `body` starts the way an event stream does, without reading the
/// rest: for a body too big to read on the UI thread.
pub fn starts_like(body: &str) -> bool {
    body.lines().find(|l| !l.is_empty()).and_then(line).is_some()
}

enum Line<'a> {
    Blank,
    Comment,
    /// A field's name and value.
    Field(&'a str, &'a str),
}

/// One line of an event stream; `None` for a line no event stream has.
fn line(text: &str) -> Option<Line<'_>> {
    if text.is_empty() {
        return Some(Line::Blank);
    }
    if text.starts_with(':') {
        return Some(Line::Comment);
    }
    // A line with no colon is a field with an empty value.
    let (name, value) = text.split_once(':').unwrap_or((text, ""));
    let value = value.strip_prefix(' ').unwrap_or(value);
    ["data", "event", "id", "retry"].contains(&name).then_some(Line::Field(name, value))
}

/// Ends an event: its payload goes on a line of its own when it is JSON.
fn push_json(data: Option<String>, out: &mut String) {
    let Some(data) = data else { return };
    if serde_json::from_str::<serde::de::IgnoredAny>(&data).is_ok() {
        out.push_str(&data);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENAI: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
                          data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n\
                          data: [DONE]\n\n";

    #[test]
    fn each_events_json_payload_is_one_line() {
        assert_eq!(
            json_payloads(OPENAI).as_deref(),
            Some(
                "{\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\
                 {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n"
            )
        );
    }

    #[test]
    fn event_and_id_fields_and_comments_are_dropped() {
        let body = ": keep-alive\n\
                    event: message_start\n\
                    id: 1\n\
                    data: {\"type\":\"message_start\"}\n\
                    \n\
                    event: ping\n\
                    retry: 3000\n\
                    data: {\"type\": \"ping\"}\n\
                    \n";
        assert_eq!(
            json_payloads(body).as_deref(),
            Some("{\"type\":\"message_start\"}\n{\"type\": \"ping\"}\n")
        );
    }

    #[test]
    fn several_data_lines_are_one_payload_joined_by_newlines() {
        let body = "data: {\"a\":\ndata: 1}\n\ndata:{\"b\":2}\n\n";
        assert_eq!(json_payloads(body).as_deref(), Some("{\"a\":\n1}\n{\"b\":2}\n"));
    }

    #[test]
    fn payloads_that_are_not_json_are_skipped() {
        let body = "data: ping\n\ndata: {\"a\":1}\n\ndata: [DONE]\n\n";
        assert_eq!(json_payloads(body).as_deref(), Some("{\"a\":1}\n"));
        assert_eq!(json_payloads("data: [DONE]\n\n").as_deref(), Some(""));
    }

    #[test]
    fn crlf_lines_and_a_last_event_with_no_blank_line_are_read() {
        let body = "data: {\"a\":1}\r\n\r\ndata: {\"a\":2}";
        assert_eq!(json_payloads(body).as_deref(), Some("{\"a\":1}\n{\"a\":2}\n"));
    }

    #[test]
    fn a_body_that_is_not_an_event_stream_is_none() {
        for body in [
            "{\"data\": 1}",
            "{\"a\":1}\n{\"a\":2}\n",
            "<html>data: 1</html>",
            "hello\ndata: {}\n",
            "event: only\nid: 1\n\n",
            "",
            "\n\n",
        ] {
            assert_eq!(json_payloads(body), None, "{body:?}");
        }
    }

    #[test]
    fn starts_like_reads_only_the_first_line_that_is_not_blank() {
        assert!(starts_like(OPENAI));
        assert!(starts_like("\nevent: x\n"));
        assert!(starts_like(": comment\n"));
        assert!(starts_like("data\n"));
        assert!(!starts_like("{\"data\": 1}"));
        assert!(!starts_like("hello\ndata: 1\n"));
        assert!(!starts_like(""));
    }
}
