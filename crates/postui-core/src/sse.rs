//! Server-sent events (`text/event-stream`), as streamed LLM APIs send
//! them, read as the stream of values their events carry (#220).
//!
//! An SSE body is not JSON, but its events' `data:` payloads usually are,
//! one value per event. [`values`] hands back what a reader's parser makes
//! of those payloads, and the jq engine and the Tree tab each read them as
//! a stream (#218): the filter
//! runs once per event, the tree stacks them. Each reader skips the
//! payloads its own parser can't read (OpenAI's closing `[DONE]`), so a
//! payload is never checked by one parser and read by another, which
//! don't accept quite the same JSON (#228).

use std::borrow::Cow;

/// Every event's data in `body`, in order, when `body` is a server-sent
/// events stream; `None` when it isn't one.
///
/// A body is an event stream by its shape, whatever its content type, as
/// a stream of JSON values is (#218): every line is a field the standard
/// names (`data`, `event`, `id`, `retry`), a `:` comment or blank, and at
/// least one is `data`. The standard ignores a field it doesn't know; this
/// reading refuses the body instead, because with no content type to go
/// on, the known names are what tell a stream from text or YAML that
/// happens to have a `data:` line, and the streams postui is for (LLM
/// APIs) send only those four.
///
/// Only the data comes back: an event's `event` name and `id` are dropped,
/// so a filter sees the payload itself (Anthropic's payloads repeat their
/// event name as `.type`). Several `data` lines in one event are one
/// payload, joined by newlines, as the standard says. Unlike the standard,
/// an event the body ends in without a blank line is kept: the body is a
/// finished response, and a stream cut short should still show what it got.
///
/// Lines end in `\n` or `\r\n`; a bare `\r` ending and a leading byte-order
/// mark, which the standard also allows, are not read, as a JSON body with
/// a byte-order mark isn't: no LLM API sends either.
fn data(body: &str) -> Option<Vec<Cow<'_, str>>> {
    // Most bodies are not event streams: tell from the first bytes, before
    // `lines` looks for the end of a first line that may be all the body.
    if !starts_like(body) {
        return None;
    }
    let mut events = Vec::new();
    let mut pending: Option<Cow<str>> = None;
    for text in body.lines() {
        match line(text)? {
            Line::Blank => events.extend(pending.take()),
            Line::Field("data", value) => match &mut pending {
                Some(d) => {
                    let d = d.to_mut();
                    d.push('\n');
                    d.push_str(value);
                }
                None => pending = Some(Cow::Borrowed(value)),
            },
            Line::Comment | Line::Field(..) => {}
        }
    }
    events.extend(pending);
    // Every `data` line makes or extends an event: none means no data.
    (!events.is_empty()).then_some(events)
}

/// The values `parse` reads from `body`'s events, in order, when `body` is
/// an event stream (see `data`); `None` when it isn't one. An event whose
/// data `parse` can't read (`[DONE]`) is skipped, so each reader skips by
/// its own parser.
pub fn values<'a, T: 'a>(
    body: &'a str,
    parse: impl Fn(&str) -> Option<T> + 'a,
) -> Option<impl Iterator<Item = T> + 'a> {
    Some(data(body)?.into_iter().filter_map(move |d| parse(&d)))
}

/// The fields the standard names; a line with any other is no event stream's.
const FIELDS: [&str; 4] = ["data", "event", "id", "retry"];

/// Whether `body` starts the way an event stream does, reading only the
/// start of its first line that is not blank: for a body too big to read
/// on the UI thread, whose first line may be all of it.
pub fn starts_like(body: &str) -> bool {
    let rest = body.trim_start_matches(['\r', '\n']);
    if rest.starts_with(':') {
        return true;
    }
    // The longest field name is five bytes; past it, the name has ended
    // or the line is no field.
    let head = &rest.as_bytes()[..rest.len().min(6)];
    let end = head.iter().position(|b| matches!(b, b':' | b'\r' | b'\n')).unwrap_or(head.len());
    let name = &head[..end];
    (end < head.len() || end == rest.len()) && FIELDS.iter().any(|f| f.as_bytes() == name)
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
    FIELDS.contains(&name).then_some(Line::Field(name, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENAI: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
                          data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n\
                          data: [DONE]\n\n";

    fn data_of(body: &str) -> Option<Vec<String>> {
        data(body).map(|d| d.into_iter().map(Cow::into_owned).collect())
    }

    #[test]
    fn each_events_data_comes_back_in_order() {
        assert_eq!(
            data_of(OPENAI).unwrap(),
            [
                "{\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}",
                "{\"choices\":[{\"delta\":{\"content\":\"!\"}}]}",
                "[DONE]",
            ]
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
            data_of(body).unwrap(),
            ["{\"type\":\"message_start\"}", "{\"type\": \"ping\"}"]
        );
    }

    #[test]
    fn several_data_lines_are_one_payload_joined_by_newlines() {
        let body = "data: {\"a\":\ndata: 1}\n\ndata:{\"b\":2}\n\n";
        assert_eq!(data_of(body).unwrap(), ["{\"a\":\n1}", "{\"b\":2}"]);
    }

    #[test]
    fn crlf_lines_and_a_last_event_with_no_blank_line_are_read() {
        let body = "data: {\"a\":1}\r\n\r\ndata: {\"a\":2}";
        assert_eq!(data_of(body).unwrap(), ["{\"a\":1}", "{\"a\":2}"]);
    }

    #[test]
    fn a_body_that_is_not_an_event_stream_is_none() {
        for body in [
            "{\"data\": 1}",
            "{\"a\":1}\n{\"a\":2}\n",
            "<html>data: 1</html>",
            "hello\ndata: {}\n",
            "name: x\ndata: 1\n",
            "event: only\nid: 1\n\n",
            "",
            "\n\n",
        ] {
            assert_eq!(data_of(body), None, "{body:?}");
        }
    }

    #[test]
    fn starts_like_reads_only_the_first_line_that_is_not_blank() {
        assert!(starts_like(OPENAI));
        assert!(starts_like("\nevent: x\n"));
        assert!(starts_like(": comment\n"));
        assert!(starts_like("data\n"));
        assert!(starts_like("data"));
        assert!(starts_like("\r\nretry: 5"));
        assert!(!starts_like("database: x"));
        assert!(!starts_like("ident"));
        assert!(!starts_like("{\"data\": 1}"));
        assert!(!starts_like("hello\ndata: 1\n"));
        assert!(!starts_like(""));
    }
}
