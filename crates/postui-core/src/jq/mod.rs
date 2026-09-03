//! The embedded jq engine (jaq) behind the response pane's filter bar.
//! jaq's own types never leave this module: callers hand in body text and
//! filter text, and get back JSON strings or a [`JqError`].

use std::ops::Range;
use std::sync::Arc;

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, data};
use jaq_json::Val;

type Data = data::JustLut<Val>;

/// Refuse to collect more than this much output text in one run — a
/// `[range(1e9)]` typo must not take the app down.
pub const OUTPUT_CAP: usize = 64 * 1024 * 1024;

/// A body parsed once by jaq's reader, shared between runs (and threads:
/// `jaq-json`'s `sync` feature makes `Val` `Arc`-backed).
#[derive(Clone)]
pub struct JqDocument(Arc<Val>);

impl std::fmt::Debug for JqDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JqDocument(..)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JqError {
    /// Lex/parse failure; `span` is a byte range into the filter text.
    Syntax { message: String, span: Option<Range<usize>> },
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
}

impl std::fmt::Display for JqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl JqDocument {
    pub fn parse(body: &str) -> Result<Self, JqError> {
        jaq_json::read::parse_single(body.as_bytes())
            .map(|v| JqDocument(Arc::new(v)))
            .map_err(|e| JqError::Runtime { message: format!("not JSON: {e}") })
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
    jaq_core::funs::<Data>().chain(jaq_std::funs()).chain(jaq_json::funs())
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

pub fn run(code: &str, doc: &JqDocument) -> Result<Vec<String>, JqError> {
    run_with_cap(code, doc, OUTPUT_CAP)
}

pub fn run_with_cap(code: &str, doc: &JqDocument, cap_bytes: usize) -> Result<Vec<String>, JqError> {
    with_compiled(code, |filter| {
        let ctx = Ctx::<Data>::new(&filter.lut, Vars::new([]));
        let mut out = Vec::new();
        let mut total = 0usize;
        for item in filter.id.run((ctx, (*doc.0).clone())) {
            let val = item.map_err(|exn| JqError::Runtime {
                message: match exn.get_err() {
                    Ok(err) => err.to_string(),
                    Err(_) => "filter broke out of its loop".to_string(),
                },
            })?;
            let text = val.to_string();
            total += text.len();
            if total > cap_bytes {
                return Err(JqError::Runtime {
                    message: format!("output too large to display (over {} bytes)", cap_bytes),
                });
            }
            out.push(text);
        }
        Ok(out)
    })
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
                return JqError::Syntax { message: msg, span: None };
            }
        }
    }
    JqError::Syntax { message: "invalid filter".into(), span: None }
}

/// Builds a `Syntax` error for an unexpected token. jaq points at the
/// offending slice `tok`; when the lexer/parser instead ran off the end of
/// the input it reports an empty `tok` with no useful position of its own,
/// so we fall back to the last character actually in `code`.
fn unexpected<S: std::fmt::Debug>(code: &str, expect: &S, tok: &str) -> JqError {
    if tok.is_empty() {
        let span = code.char_indices().last().map(|(i, c)| i..i + c.len_utf8());
        return JqError::Syntax {
            message: format!("unexpected end of filter — expected {}", expect_text(expect)),
            span,
        };
    }
    JqError::Syntax {
        message: format!("unexpected `{tok}` — expected {}", expect_text(expect)),
        span: span_of(code, tok),
    }
}

fn expect_text<S: std::fmt::Debug>(expect: &S) -> String {
    // Neither `Expect` type has a `Display`; their Debug names are fine
    // once lower-cased (`Nothing`, `Delim("(")`, `Term`, ...).
    format!("{expect:?}").to_lowercase()
}

fn compile_error(code: &str, errs: jaq_core::compile::Errors<&str, ()>) -> JqError {
    use jaq_core::compile::Undefined;
    for (_, undefined) in errs {
        if let Some((name, kind)) = undefined.into_iter().next() {
            let span = span_of(code, name);
            return match kind {
                Undefined::Filter(arity) => JqError::Unknown { name: name.to_string(), arity, span },
                other => JqError::Syntax { message: format!("undefined {}: `{name}`", other.as_str()), span },
            };
        }
    }
    JqError::Syntax { message: "invalid filter".into(), span: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"data":{"items":[{"id":1,"name":"a","status":"active"},{"id":2,"name":"b","status":"off"}],"total":2}}"#;

    fn doc() -> JqDocument {
        JqDocument::parse(DOC).expect("fixture is JSON")
    }

    #[test]
    fn a_filter_runs_and_returns_compact_json_outputs() {
        let out = run(".data.items | length", &doc()).unwrap();
        assert_eq!(out, vec!["2"]);
        let out = run(r#".data.items | map(select(.status == "active")) | map(.name)"#, &doc()).unwrap();
        assert_eq!(out, vec![r#"["a"]"#]);
    }

    #[test]
    fn every_output_of_a_multi_output_filter_is_returned_in_order() {
        let out = run(".data.items[] | .id", &doc()).unwrap();
        assert_eq!(out, vec!["1", "2"]);
    }

    #[test]
    fn a_syntax_error_names_the_offending_token_with_its_span() {
        let err = check(".foo | select(").unwrap_err();
        let JqError::Syntax { span, .. } = &err else {
            panic!("expected a syntax error, got {err:?}");
        };
        let span = span.clone().expect("lex/parse errors carry a span");
        assert_eq!(&".foo | select("[span.clone()], "(", "span covers the token: {span:?}");
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
    fn a_runtime_error_has_a_readable_message_and_no_partial_output() {
        let err = run(".data.items[] | .id | .foo", &doc()).unwrap_err();
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
        let err = run(".data.total | .x", &doc()).unwrap_err();
        assert!(err.span().is_none(), "runtime errors have no span");
    }
}
