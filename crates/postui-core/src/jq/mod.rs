//! The embedded jq engine (jaq) behind the response pane's filter bar.
//! jaq's own types never leave this module: callers hand in body text and
//! filter text, and get back JSON strings or a [`JqError`].

use std::ops::{ControlFlow, Range};
use std::sync::Arc;

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, DataT, Lut, Vars, data::HasLut};
use jaq_json::Val;
use jaq_std::input::{HasInputs, Inputs, RcIter};

pub mod ai;
pub mod complete;
pub mod path;
pub mod shape;
pub use path::{PathSeg, compose, render_path};

/// What a filter runs on: JSON values, with the compiled lookup table and
/// the values `input`/`inputs` read (`Globals`) as its global data.
struct Data;

impl DataT for Data {
    type V<'a> = Val;
    type Data<'a> = &'a Globals<'a>;
}

struct Globals<'a> {
    lut: &'a Lut<Data>,
    inputs: Inputs<'a, Val>,
}

impl<'a> HasLut<'a, Data> for &'a Globals<'a> {
    fn lut(&self) -> &'a Lut<Data> {
        self.lut
    }
}

impl<'a> HasInputs<'a, Val> for &'a Globals<'a> {
    fn inputs(&self) -> Inputs<'a, Val> {
        self.inputs
    }
}

/// Refuse to collect more than this much output text in one run — a
/// `[range(1e9)]` typo must not take the app down.
pub const OUTPUT_CAP: usize = 64 * 1024 * 1024;

/// A body parsed once by jaq's reader, shared between runs (and threads:
/// `jaq-json`'s `sync` feature makes `Val` `Arc`-backed). Every JSON value
/// in the body, in order: one for a JSON body, several for a stream such as
/// NDJSON, which a filter runs over the way jq runs over its inputs
/// ([`for_each_output`]).
#[derive(Clone)]
pub struct JqDocument(Arc<[Val]>);

impl std::fmt::Debug for JqDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JqDocument(..)")
    }
}

/// Identity, not structural, equality — `Action` derives `PartialEq`/`Eq`
/// and a document is only ever compared as "the same parse or not".
impl PartialEq for JqDocument {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for JqDocument {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JqError {
    /// Lex/parse failure; `span` is a byte range into the filter text.
    /// `incomplete` means the parser ran off the end of the input — the
    /// filter is unfinished rather than wrong, so there is no span: no
    /// character typed so far is to blame.
    Syntax { message: String, span: Option<Range<usize>>, incomplete: bool },
    /// `nosuchfn/1` — name and arity, with the call's span when known.
    Unknown { name: String, arity: usize, span: Option<Range<usize>> },
    /// A runtime error from jaq (`cannot index number with "foo"`), a body
    /// that is not JSON, or output past the cap.
    Runtime { message: String },
}

impl JqError {
    pub fn message(&self) -> String {
        match self {
            JqError::Syntax { message, .. } | JqError::Runtime { message } => message.clone(),
            JqError::Unknown { name, arity, .. } => format!("unknown function {name}/{arity}"),
        }
    }

    pub fn span(&self) -> Option<Range<usize>> {
        match self {
            JqError::Syntax { span, .. } | JqError::Unknown { span, .. } => span.clone(),
            JqError::Runtime { .. } => None,
        }
    }

    /// Whether the filter merely stops short (an unclosed bracket, a
    /// trailing `|`) — the normal state of one still being typed.
    pub fn incomplete(&self) -> bool {
        matches!(self, JqError::Syntax { incomplete: true, .. })
    }
}

impl std::fmt::Display for JqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl JqDocument {
    /// Every whitespace-separated JSON value in `body`, as jq reads its
    /// input. A body with no value, or with a bad one anywhere, is not JSON.
    /// A server-sent events body is the stream of its events' data instead
    /// ([`crate::sse::values`], #220), and there an event whose data isn't a
    /// JSON value (`[DONE]`) is skipped rather than failing the body.
    pub fn parse(body: &str) -> Result<Self, JqError> {
        let event = |d: &str| jaq_json::read::parse_single(d.as_bytes()).ok();
        if let Some(values) = crate::sse::values(body, event) {
            if values.is_empty() {
                return Err(JqError::Runtime {
                    message: "not JSON: no event's data is a JSON value".into(),
                });
            }
            return Ok(JqDocument(values.into()));
        }
        let not_json = |e| JqError::Runtime { message: format!("not JSON: {e}") };
        let values: Vec<Val> = jaq_json::read::parse_many(body.as_bytes())
            .collect::<Result<_, _>>()
            .map_err(not_json)?;
        if values.is_empty() {
            return Err(JqError::Runtime { message: "not JSON: the body holds no value".into() });
        }
        Ok(JqDocument(values.into()))
    }
}

/// The byte range of `token` inside `code`, when `token` is a sub-slice of
/// it (jaq's lex/parse errors point at slices of the source).
fn span_of(code: &str, token: &str) -> Option<Range<usize>> {
    let base = code.as_ptr() as usize;
    let at = token.as_ptr() as usize;
    if at < base || at + token.len() > base + code.len() {
        return None;
    }
    let start = at - base;
    Some(start..start + token.len())
}

fn defs() -> impl Iterator<Item = jaq_core::load::parse::Def<&'static str>> {
    jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs())
}

fn funs() -> impl Iterator<Item = jaq_core::native::Fun<Data>> {
    let inputs = jaq_std::input::funs().into_vec().into_iter().map(jaq_core::native::run);
    jaq_core::funs::<Data>().chain(jaq_std::funs()).chain(inputs).chain(jaq_json::funs())
}

/// Compiles `code` against a fresh arena and hands the compiled filter to
/// `then`. Filters borrow the arena, so this is the only shape that lets a
/// caller use one without a self-referential struct.
fn with_compiled<T>(
    code: &str,
    then: impl FnOnce(&jaq_core::Filter<Data>) -> Result<T, JqError>,
) -> Result<T, JqError> {
    let arena = Arena::default();
    let modules = Loader::new(defs())
        .load(&arena, File { code, path: () })
        .map_err(|errs| load_error(code, errs))?;
    let filter = Compiler::default()
        .with_funs(funs())
        .compile(modules)
        .map_err(|errs| compile_error(code, errs))?;
    then(&filter)
}

pub fn check(code: &str) -> Result<(), JqError> {
    with_compiled(code, |_| Ok(()))
}

/// What a filter made of a document: every output, as compact JSON in
/// order, and beside them the error of a value whose run failed. As in jq,
/// an error ends only the run of the value it happened on, so the outputs
/// are the other values' and those printed before it. When more than one
/// value's run failed, `error` is the first one's, its message starting
/// `first of N errors: ` (N counts failed runs: a filter reading
/// `input`/`inputs` takes several values in one): one line for the bar's one row and for an agent reading
/// `run --jq`, where jq prints a line per failing value (#229 spec, R3).
/// The count leads because jaq's message holds the failing value, which
/// on a long line runs past the bar's row, and it says "first" because
/// the other values may have failed differently. It is part of the
/// message, not a field of its own, so the bar and the CLI say it the
/// same way from this one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JqRun {
    pub outputs: Vec<String>,
    pub error: Option<JqError>,
}

/// Runs `code` over `doc`. `Err` is for what fails the whole run: a filter
/// that doesn't compile, or output past the cap. A filter error on a value
/// is the run's [`JqRun::error`], beside the outputs.
pub fn run(code: &str, doc: &JqDocument) -> Result<JqRun, JqError> {
    run_with_cap(code, doc, OUTPUT_CAP)
}

pub fn run_with_cap(code: &str, doc: &JqDocument, cap_bytes: usize) -> Result<JqRun, JqError> {
    with_compiled(code, |filter| {
        let mut outputs = Vec::new();
        let mut total = 0usize;
        let mut first = None;
        let mut failed = 0usize;
        for_each_output(filter, doc, |item| {
            match item {
                Ok(value) => {
                    let text = value.to_string();
                    total += text.len();
                    if total > cap_bytes {
                        return Err(JqError::Runtime {
                            message: format!(
                                "output too large to display (over {} bytes)",
                                cap_bytes
                            ),
                        });
                    }
                    outputs.push(text);
                }
                // `for_each_output` ends the value's run on its error, so
                // each one is a value that failed. Every one has had its
                // message formatted (jaq's holds the value), though only
                // the first is kept: a filter failing on every line of a
                // big stream formats the whole body, where the run used to
                // stop at the first error. Accepted to keep the error
                // handling simple (the user, #229); jq formats every error
                // too.
                Err(e) => {
                    failed += 1;
                    first.get_or_insert(e);
                }
            }
            Ok(ControlFlow::Continue(()))
        })?;
        let error = first.map(|e| match failed {
            1 => e,
            n => JqError::Runtime { message: format!("first of {n} errors: {}", e.message()) },
        });
        Ok(JqRun { outputs, error })
    })
}

/// Runs `filter` over `doc` the way jq runs a program over its inputs:
/// once per value, in order, with `input`/`inputs` taking the values after
/// the current one (so a run that reads them all is the last). `each` sees
/// every output in order, a filter error included, and may stop the run
/// early (`Break`) or end it with an error. An error it lets go
/// (`Continue`) ends only that value's run, as in jq, and the run goes on
/// with the next value: [`run`] and completion both do that, so one odd
/// line in a stream hides neither the results nor the keys of the rest.
fn for_each_output(
    filter: &jaq_core::Filter<Data>,
    doc: &JqDocument,
    mut each: impl FnMut(Result<Val, JqError>) -> Result<ControlFlow<()>, JqError>,
) -> Result<(), JqError> {
    let values = RcIter::new(doc.0.iter().cloned().map(Ok));
    let globals = Globals { lut: &filter.lut, inputs: &values };
    while let Some(Ok(input)) = (&values).next() {
        for item in filter.id.run((Ctx::<Data>::new(&globals, Vars::new([])), input)) {
            let failed = item.is_err();
            let item = item.map_err(|exn| JqError::Runtime {
                message: match exn.get_err() {
                    Ok(err) => err.to_string(),
                    Err(_) => "filter broke out of its loop".to_string(),
                },
            });
            if each(item)?.is_break() {
                return Ok(());
            }
            if failed {
                break;
            }
        }
    }
    Ok(())
}

fn load_error(code: &str, errs: jaq_core::load::Errors<&str, ()>) -> JqError {
    use jaq_core::load::Error;
    for (_, err) in errs {
        match err {
            Error::Lex(v) => {
                if let Some((expect, tok)) = v.into_iter().next() {
                    return unexpected(code, &expect, tok);
                }
            }
            Error::Parse(v) => {
                if let Some((expect, tok)) = v.into_iter().next() {
                    return unexpected(code, &expect, tok);
                }
            }
            Error::Io(v) => {
                let msg = v.into_iter().next().map(|(_, e)| e).unwrap_or_default();
                return JqError::Syntax { message: msg, span: None, incomplete: false };
            }
        }
    }
    JqError::Syntax { message: "invalid filter".into(), span: None, incomplete: false }
}

/// Builds a `Syntax` error for an unexpected token. jaq points at the
/// offending slice `tok`; when the lexer/parser instead ran off the end of
/// the input it reports an empty `tok` — the filter is unfinished, which
/// is what every filter looks like mid-typing, so the error says what is
/// missing, blames no character, and is marked `incomplete`.
fn unexpected<S: std::fmt::Debug>(code: &str, expect: &S, tok: &str) -> JqError {
    if tok.is_empty() {
        return JqError::Syntax { message: incomplete_text(expect), span: None, incomplete: true };
    }
    JqError::Syntax {
        message: format!("unexpected `{tok}` — expected {}", expect_text(expect)),
        span: span_of(code, tok),
        incomplete: false,
    }
}

fn expect_text<S: std::fmt::Debug>(expect: &S) -> String {
    // Neither `Expect` type has a `Display`; their Debug names are fine
    // once lower-cased (`Nothing`, `Delim("(")`, `Term`, ...).
    format!("{expect:?}").to_lowercase()
}

/// What an unfinished filter is missing, from the parser's expectation:
/// `Delim("(")` names the opener left unclosed, `Term` a missing operand
/// (`.a |`, `if .a then`). Anything else keeps jaq's own name.
fn incomplete_text<S: std::fmt::Debug>(expect: &S) -> String {
    let debug = format!("{expect:?}");
    if let Some(opener) = debug.strip_prefix("Delim(\"").and_then(|s| s.strip_suffix("\")")) {
        // The escaped quote is `\"` in Debug; unescape it.
        return format!("unclosed {}", opener.replace("\\\"", "\""));
    }
    if debug == "Term" {
        return "expected a term".into();
    }
    format!("expected {}", debug.to_lowercase())
}

fn compile_error(code: &str, errs: jaq_core::compile::Errors<&str, ()>) -> JqError {
    use jaq_core::compile::Undefined;
    for (_, undefined) in errs {
        if let Some((name, kind)) = undefined.into_iter().next() {
            let span = span_of(code, name);
            return match kind {
                Undefined::Filter(arity) => {
                    JqError::Unknown { name: name.to_string(), arity, span }
                }
                other => JqError::Syntax {
                    message: format!("undefined {}: `{name}`", other.as_str()),
                    span,
                    incomplete: false,
                },
            };
        }
    }
    JqError::Syntax { message: "invalid filter".into(), span: None, incomplete: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"data":{"items":[{"id":1,"name":"a","status":"active"},{"id":2,"name":"b","status":"off"}],"total":2}}"#;

    fn doc() -> JqDocument {
        JqDocument::parse(DOC).expect("fixture is JSON")
    }

    /// The outputs of a run that must not fail.
    fn outputs(code: &str, doc: &JqDocument) -> Vec<String> {
        let out = run(code, doc).unwrap();
        assert_eq!(out.error, None, "{code:?}");
        out.outputs
    }

    /// The error of a run that compiles but fails on its value.
    fn runtime_error(code: &str, doc: &JqDocument) -> JqError {
        run(code, doc).unwrap().error.expect("the run fails")
    }

    #[test]
    fn a_filter_runs_and_returns_compact_json_outputs() {
        assert_eq!(outputs(".data.items | length", &doc()), vec!["2"]);
        let out = outputs(r#".data.items | map(select(.status == "active")) | map(.name)"#, &doc());
        assert_eq!(out, vec![r#"["a"]"#]);
    }

    #[test]
    fn every_output_of_a_multi_output_filter_is_returned_in_order() {
        assert_eq!(outputs(".data.items[] | .id", &doc()), vec!["1", "2"]);
    }

    #[test]
    fn a_syntax_error_names_the_offending_token_with_its_span() {
        let err = check(".foo )").unwrap_err();
        let JqError::Syntax { span, incomplete, .. } = &err else {
            panic!("expected a syntax error, got {err:?}");
        };
        assert!(!incomplete, "a stray token is a real error");
        let span = span.clone().expect("lex/parse errors carry a span");
        assert_eq!(&".foo )"[span.clone()], ")", "span covers the token: {span:?}");
        assert!(!err.incomplete());
    }

    #[test]
    fn running_off_the_end_of_the_filter_is_incomplete_with_no_span() {
        for (code, want) in [
            (".foo | select(", "unclosed ("),
            ("map({name", "unclosed {"),
            ("[.a", "unclosed ["),
            (".a | \"abc", "unclosed \""),
            (".a |", "expected a term"),
            ("if .a then", "expected a term"),
        ] {
            let err = check(code).unwrap_err();
            assert!(err.incomplete(), "{code:?} should be incomplete: {err:?}");
            assert_eq!(err.span(), None, "{code:?}: an unfinished filter blames no character");
            assert_eq!(err.message(), want, "for {code:?}");
        }
    }

    #[test]
    fn only_end_of_input_syntax_errors_are_incomplete() {
        assert!(!check("this is not jq").unwrap_err().incomplete());
        assert!(!check(".foo | nosuchfn(1)").unwrap_err().incomplete());
        assert!(!runtime_error(".data.total | .x", &doc()).incomplete());
    }

    #[test]
    fn an_unknown_function_is_reported_by_name_and_arity_with_its_span() {
        let err = check(".foo | nosuchfn(1)").unwrap_err();
        let JqError::Unknown { name, arity, span } = &err else {
            panic!("expected an unknown-function error, got {err:?}");
        };
        assert_eq!(name, "nosuchfn");
        assert_eq!(*arity, 1);
        let span = span.clone().expect("unknown-function errors carry a span");
        assert_eq!(&".foo | nosuchfn(1)"[span], "nosuchfn");
    }

    #[test]
    fn a_runtime_error_has_a_readable_message() {
        let out = run(".data.items[] | .id | .foo", &doc()).unwrap();
        assert!(out.outputs.is_empty(), "the first output already fails: {out:?}");
        let err = out.error.expect("the run fails");
        let JqError::Runtime { message } = &err else {
            panic!("expected a runtime error, got {err:?}");
        };
        assert!(message.contains("cannot index"), "message: {message}");
    }

    #[test]
    fn non_json_bodies_are_rejected_at_parse() {
        let err = JqDocument::parse("<html>").unwrap_err();
        assert!(matches!(err, JqError::Runtime { .. }), "got {err:?}");
    }

    const STREAM: &str = "{\"a\":1,\"b\":\"x\"}\n{\"a\":2}\n{\"a\":3,\"c\":true}\n";

    #[test]
    fn a_stream_body_runs_the_filter_once_per_value_like_jq() {
        let doc = JqDocument::parse(STREAM).expect("NDJSON is a stream of values");
        assert_eq!(outputs(".a", &doc), vec!["1", "2", "3"]);
        assert_eq!(
            outputs(".", &JqDocument::parse("1 [2] \"x\"").unwrap()),
            vec!["1", "[2]", "\"x\""]
        );
    }

    #[test]
    fn inputs_reads_the_values_after_the_current_one() {
        let doc = JqDocument::parse(STREAM).unwrap();
        assert_eq!(outputs("[., inputs] | map(.a)", &doc), vec!["[1,2,3]"]);
        // `input` takes the next value, so the filter's next run starts after it.
        let four = JqDocument::parse("1 2 3 4").unwrap();
        assert_eq!(outputs("[., input]", &four), vec!["[1,2]", "[3,4]"]);
    }

    #[test]
    fn inputs_on_a_single_value_body_yield_nothing() {
        assert_eq!(outputs("[inputs]", &doc()), vec!["[]"]);
    }

    #[test]
    fn an_error_ends_only_its_values_run_and_the_others_still_come_out() {
        // The middle line has no `items`, as an Ollama-style stream's last
        // line often doesn't.
        let doc = JqDocument::parse(
            "{\"items\":[{\"id\":1}]}\n{\"done\":true}\n{\"items\":[{\"id\":2}]}\n",
        )
        .unwrap();
        let out = run(".items[] | .id", &doc).unwrap();
        assert_eq!(out.outputs, vec!["1", "2"]);
        let err = out.error.expect("the failing value's error is reported with them");
        assert!(matches!(err, JqError::Runtime { .. }), "got {err:?}");
        assert!(err.message().contains("null as iterable"), "{err:?}");
        assert!(!err.message().contains("errors:"), "one value failed: {err:?}");
    }

    #[test]
    fn a_values_outputs_before_its_error_are_kept_as_jq_prints_them() {
        let doc = JqDocument::parse(r#"[{"a":1},2,{"a":3}]"#).unwrap();
        let out = run(".[] | .a", &doc).unwrap();
        assert_eq!(out.outputs, vec!["1"], "the run of the one value ends at its error");
        assert!(out.error.is_some());
    }

    #[test]
    fn several_failing_values_report_the_first_error_with_their_count() {
        let doc = JqDocument::parse("{\"a\":1}\n2\n{\"a\":3}\n\"x\"\n").unwrap();
        let out = run(".a", &doc).unwrap();
        assert_eq!(out.outputs, vec!["1", "3"]);
        let first = run(".a", &JqDocument::parse("2").unwrap()).unwrap().error.unwrap();
        let err = out.error.unwrap();
        assert_eq!(err.message(), format!("first of 2 errors: {}", first.message()));
    }

    #[test]
    fn a_run_with_no_error_has_none() {
        let out = run(".a", &JqDocument::parse(STREAM).unwrap()).unwrap();
        assert_eq!(out, JqRun { outputs: vec!["1".into(), "2".into(), "3".into()], error: None });
    }

    #[test]
    fn the_cap_ends_the_whole_run_even_with_outputs_before_it() {
        let doc = JqDocument::parse("1 2 3").unwrap();
        let err = run_with_cap("if . == 3 then [range(1000)] else . end", &doc, 1024).unwrap_err();
        assert!(err.message().contains("too large"), "{err:?}");
    }

    #[test]
    fn an_event_stream_runs_the_filter_once_per_json_payload() {
        let body = "event: delta\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n\
                    data: [DONE]\n\n";
        let doc = JqDocument::parse(body).expect("an SSE body is a stream of its payloads");
        assert_eq!(outputs(".choices[0].delta.content", &doc), vec!["\"Hi\"", "\"!\""]);
        let err = JqDocument::parse("data: [DONE]\n\ndata: 1 2\n\n").unwrap_err();
        assert_eq!(err.message(), "not JSON: no event's data is a JSON value");
    }

    #[test]
    fn a_bad_value_anywhere_in_a_stream_is_not_json() {
        let err = JqDocument::parse("{\"a\":1}\n{oops}\n").unwrap_err();
        assert!(err.message().starts_with("not JSON: byte offset"), "got {err:?}");
        for empty in ["", " \n"] {
            assert!(JqDocument::parse(empty).is_err(), "{empty:?} has no value");
        }
    }

    #[test]
    fn output_past_the_cap_is_refused_rather_than_collected() {
        let err = run_with_cap("[range(100000)]", &doc(), 1024).unwrap_err();
        let JqError::Runtime { message } = &err else {
            panic!("expected a runtime error, got {err:?}");
        };
        assert!(message.contains("too large"), "message: {message}");
    }

    #[test]
    fn a_document_can_cross_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<JqDocument>();
    }

    #[test]
    fn errors_expose_a_message_and_span_uniformly() {
        let err = check("this is not jq").unwrap_err();
        assert!(!err.message().is_empty());
        assert!(err.span().is_some());
        let err = runtime_error(".data.total | .x", &doc());
        assert!(err.span().is_none(), "runtime errors have no span");
    }
}
