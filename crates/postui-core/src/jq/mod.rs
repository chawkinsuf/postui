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
    stop: &'a Stop<'a>,
}

/// jaq calls this on every term it evaluates (`Id::run`), so a filter that
/// loops through jq definitions passes here on each step, output or not:
/// the point where a run past its limit is stopped (#310 spec, §1).
impl<'a> HasLut<'a, Data> for &'a Globals<'a> {
    fn lut(&self) -> &'a Lut<Data> {
        self.stop.tick();
        self.lut
    }
}

/// Whether a run is stopped (#310). [`Stop::tick`] is called on each step
/// of the run; once the flag, or `quit`, is set it unwinds out of jaq with
/// a [`Stopped`], which [`for_each_output`] catches. [`with_limit`] makes
/// the one each run has.
#[derive(Default)]
struct Stop<'a> {
    stopped: std::sync::atomic::AtomicBool,
    /// The limit that sets the flag, for the message.
    limit: std::time::Duration,
    /// A flag the run's caller sets to stop it before its limit: the app's,
    /// set as it quits (#308 spec B7).
    quit: Option<&'a std::sync::atomic::AtomicBool>,
}

/// What a stopped run unwinds with. `resume_unwind` skips the panic hook,
/// so nothing is printed (the app's hook would print on the TUI's screen),
/// and a filter's `try`, `?` and `//` can't catch it, since it is not a
/// jaq error.
struct Stopped(std::time::Duration);

impl Stop<'_> {
    /// One step of the run: unwinds with [`Stopped`] once the flag or
    /// `quit` is set.
    fn tick(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.stopped.load(Relaxed) || self.quit.is_some_and(|quit| quit.load(Relaxed)) {
            std::panic::resume_unwind(Box::new(Stopped(self.limit)));
        }
    }
}

/// Runs `run` with a [`Stop`] whose flag a timer thread sets once `limit`
/// has passed, unless `run` ends first, and which `quit` stops too. The
/// run reads a flag on each step rather than the clock, so a run whose
/// steps are each slow still stops one step past the limit. Costs a thread
/// for each run: a timer of its own rather than one shared by every run, so
/// no state outlives the run. The limit is wall-clock time, so a run slowed
/// by other work on the machine counts that time too.
fn with_limit<T>(
    limit: std::time::Duration,
    quit: Option<&std::sync::atomic::AtomicBool>,
    run: impl FnOnce(&Stop) -> T,
) -> T {
    use std::sync::mpsc::{RecvTimeoutError, channel};
    let stop = Stop { limit, quit, ..Stop::default() };
    let (ended, wait) = channel::<()>();
    std::thread::scope(|scope| {
        let stop = &stop;
        scope.spawn(move || {
            if let Err(RecvTimeoutError::Timeout) = wait.recv_timeout(limit) {
                stop.stopped.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        // Dropped as `run` returns or unwinds, which ends the timer's wait.
        let _ended = ended;
        run(stop)
    })
}

impl<'a> HasInputs<'a, Val> for &'a Globals<'a> {
    fn inputs(&self) -> Inputs<'a, Val> {
        self.inputs
    }
}

/// Refuse to collect more than this much output text in one run — a
/// `[range(1e9)]` typo must not take the app down.
pub const OUTPUT_CAP: usize = 64 * 1024 * 1024;

/// The deepest nesting of arrays and objects a body is read with (#248,
/// `mine`). jaq reads a value, and prints it, by recursing once per level
/// with no limit, and a stack overflow aborts the process. Measured on a
/// 2 MB stack with an object body, debug / release build: the reader
/// overflowed from about 1,300 / 4,600 levels, printing with `.` from
/// about 400 / 2,000. What a filter does with the value is not covered
/// (#282).
const MAX_DEPTH: usize = 256;

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
    /// The run was stopped at `limit` (#310, #308).
    Stopped { limit: std::time::Duration },
}

impl JqError {
    pub fn message(&self) -> String {
        match self {
            JqError::Syntax { message, .. } | JqError::Runtime { message } => message.clone(),
            JqError::Unknown { name, arity, .. } => format!("unknown function {name}/{arity}"),
            JqError::Stopped { limit } => format!("stopped at the {} ms limit", limit.as_millis()),
        }
    }

    pub fn span(&self) -> Option<Range<usize>> {
        match self {
            JqError::Syntax { span, .. } | JqError::Unknown { span, .. } => span.clone(),
            JqError::Runtime { .. } | JqError::Stopped { .. } => None,
        }
    }

    /// The limit the run was stopped at, when this is that stop.
    pub fn stopped_at(&self) -> Option<std::time::Duration> {
        match self {
            JqError::Stopped { limit } => Some(*limit),
            _ => None,
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
    ///
    /// A body serde refuses (`strict_json`), the reader the response's Tree
    /// tab is built with, is not JSON here either, so the Tree, the jq bar
    /// and `postui run --jq` agree on comments and NaN (#228, strict JSON:
    /// `mine (approved)`). jaq's own reader also takes `#` comments and
    /// NaN/Infinity.
    ///
    /// A body nested past [`MAX_DEPTH`] is refused, and an event that deep
    /// is skipped like one that isn't JSON (#248, `too_deep`).
    pub fn parse(body: &str) -> Result<Self, JqError> {
        // One value per event, as the Tree reads an event.
        let event = |d: &str| {
            if too_deep(d) {
                return None;
            }
            serde_json::from_str::<serde::de::IgnoredAny>(d).ok()?;
            jaq_json::read::parse_single(d.as_bytes()).ok()
        };
        if let Some(values) = crate::sse::values(body, event) {
            let values: Vec<Val> = values.collect();
            if values.is_empty() {
                return Err(JqError::Runtime {
                    message: "not JSON: no event's data is a JSON value".into(),
                });
            }
            return Ok(JqDocument(values.into()));
        }
        fn not_json(e: impl std::fmt::Display) -> JqError {
            JqError::Runtime { message: format!("not JSON: {e}") }
        }
        if too_deep(body) {
            return Err(JqError::Runtime {
                message: format!("the body is nested deeper than {MAX_DEPTH} levels"),
            });
        }
        let values: Vec<Val> = jaq_json::read::parse_many(body.as_bytes())
            .collect::<Result<_, _>>()
            .map_err(not_json)?;
        if values.is_empty() {
            return Err(JqError::Runtime { message: "not JSON: the body holds no value".into() });
        }
        strict_json(body).map_err(not_json)?;
        Ok(JqDocument(values.into()))
    }
}

/// Whether serde reads `text` as whitespace-separated JSON values, without
/// building them: a check pass beside jaq's read, about 3% of its time and
/// no memory on a 45 MB body. It catches what jaq takes and serde doesn't,
/// in the bodies servers send: `#` comments, NaN/Infinity. It is not a
/// copy of the Tree's reader: a body nested past serde's limit (which
/// `IgnoredAny` skips without) or one using serde's private number key can
/// still differ from the Tree. Kept that way on purpose (#228): matching
/// the Tree exactly meant reading with serde, 40–110% slower on large
/// bodies, for cases this rare (you: "I'm not sure I want to make things as
/// complex as this seems to be for some rare edge cases"; this shape:
/// `mine (approved)`).
fn strict_json(text: &str) -> Result<(), serde_json::Error> {
    serde_json::Deserializer::from_str(text)
        .into_iter::<serde::de::IgnoredAny>()
        .try_for_each(|value| value.map(drop))
}

/// Whether `text` nests arrays and objects past [`MAX_DEPTH`], found without
/// recursion and before jaq's reader runs. One more pass over the body:
/// 13 to 19 ms on a 45 MB JSON one, measured.
///
/// It counts the brackets jaq's reader would take as nesting, so it skips
/// what that reader skips: strings and `#` comments. On text that isn't
/// JSON the count may be off, and such a body is not JSON either way.
fn too_deep(text: &str) -> bool {
    let mut depth = 0usize;
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' => loop {
                match bytes.next() {
                    None | Some(b'"') => break,
                    // The escaped byte, a quote included, ends no string.
                    Some(b'\\') => {
                        bytes.next();
                    }
                    Some(_) => {}
                }
            },
            b'#' => {
                bytes.find(|&b| b == b'\n');
            }
            _ => {}
        }
    }
    false
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

/// jaq's native filters, with its `range/3` replaced by [`range`].
fn funs() -> impl Iterator<Item = jaq_core::native::Fun<Data>> {
    let inputs = jaq_std::input::funs().into_vec().into_iter().map(jaq_core::native::run);
    let core =
        jaq_core::funs::<Data>().filter(|(name, args, _)| (*name, args.len()) != ("range", 3));
    let range = jaq_core::native::run::<Data>(("range", jaq_core::native::v(3), range));
    core.chain([range]).chain(jaq_std::funs()).chain(inputs).chain(jaq_json::funs())
}

/// `range($from; $to; $by)`, jaq's own (`jaq-core-3.1.1/src/funs.rs`,
/// `range`), with a [`Stop::tick`] on each value: jaq's is a plain Rust
/// loop that never passes through `lut()`, so `last(range(1e18))` would
/// not be stopped (#310 spec, §1). `range/1` and `range/2` are defined
/// on top of it. A copy rather than a wrapper around jaq's: jaq-core keeps
/// its `range` and a native filter's function private, so there is
/// nothing to call.
fn range<'a>(mut cv: jaq_core::Cv<'a, Data>) -> jaq_core::ValXs<'a, Val> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let by = cv.0.pop_var();
    let to = cv.0.pop_var();
    let mut from: jaq_core::ValX<'a, Val> = Ok(cv.0.pop_var());
    let stop = cv.0.data().stop;
    let cmp = by.partial_cmp(&Val::from(0usize)).unwrap_or(Equal);
    Box::new(std::iter::from_fn(move || {
        stop.tick();
        match from.clone() {
            Ok(x) => match cmp {
                Greater => x < to,
                Less => x > to,
                Equal => x != to,
            }
            .then(|| std::mem::replace(&mut from, (x + by.clone()).map_err(jaq_core::Exn::from))),
            e @ Err(_) => {
                // Nothing after the error.
                from = Ok(to.clone());
                Some(e)
            }
        }
    }))
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

/// Runs `code` over `doc`, stopped once `limit` has passed since the filter
/// compiled: the project's `jq_limit_ms`, for checks (#310) and for the jq
/// bar, completion and `run --jq` alike (#308). `Err` is for what fails the
/// whole run: a filter that doesn't compile, output past the cap, or the
/// limit (`stopped at the <N> ms limit`). A filter error on a value is the
/// run's [`JqRun::error`], beside the outputs.
///
/// One limit covers the whole run, every value of a stream body included,
/// as one jq program runs over all its inputs. Checked between the
/// filter's steps, so one native step already running (a huge allocation,
/// a sort) finishes first.
///
/// Once `quit` is set the run stops the same way, at its next step, and
/// fails with the same error: the app sets it as it quits, so that no run
/// holds the process up (#308 spec B7).
pub fn run(
    code: &str,
    doc: &JqDocument,
    limit: std::time::Duration,
    quit: Option<&std::sync::atomic::AtomicBool>,
) -> Result<JqRun, JqError> {
    run_with_cap(code, doc, OUTPUT_CAP, limit, quit)
}

/// [`run`], refused past `cap_bytes` of output.
pub fn run_with_cap(
    code: &str,
    doc: &JqDocument,
    cap_bytes: usize,
    limit: std::time::Duration,
    quit: Option<&std::sync::atomic::AtomicBool>,
) -> Result<JqRun, JqError> {
    with_compiled(code, |filter| {
        with_limit(limit, quit, |stop| collect(filter, doc, cap_bytes, stop))
    })
}

/// The [`JqRun`] of `filter` over `doc`, refused past `cap_bytes` of output.
fn collect(
    filter: &jaq_core::Filter<Data>,
    doc: &JqDocument,
    cap_bytes: usize,
    stop: &Stop,
) -> Result<JqRun, JqError> {
    let mut outputs = Vec::new();
    let mut total = 0usize;
    let mut first = None;
    let mut failed = 0usize;
    for_each_output(filter, doc, stop, |item| {
        match item {
            Ok(value) => {
                let text = value.to_string();
                total += text.len();
                if total > cap_bytes {
                    return Err(JqError::Runtime {
                        message: format!("output too large to display (over {} bytes)", cap_bytes),
                    });
                }
                outputs.push(text);
            }
            // `for_each_output` ends the value's run on its error, so each
            // one is a value that failed. Every one has had its message
            // formatted (jaq's holds the value), though only the first is
            // kept: a filter failing on every line of a big stream formats
            // the whole body, where the run used to stop at the first
            // error. Accepted to keep the error handling simple (the user,
            // #229); jq formats every error too.
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
}

/// Runs `filter` over `doc` the way jq runs a program over its inputs:
/// once per value, in order, with `input`/`inputs` taking the values after
/// the current one (so a run that reads them all is the last). `each` sees
/// every output in order, a filter error included, and may stop the run
/// early (`Break`) or end it with an error. An error it lets go
/// (`Continue`) ends only that value's run, as in jq, and the run goes on
/// with the next value: [`run`] and completion both do that, so one odd
/// line in a stream hides neither the results nor the keys of the rest.
///
/// A run whose `stop` is set ends whole with its error (#310): the unwind
/// from [`Stop::tick`] is caught here, and any other panic goes on.
fn for_each_output(
    filter: &jaq_core::Filter<Data>,
    doc: &JqDocument,
    stop: &Stop,
    mut each: impl FnMut(Result<Val, JqError>) -> Result<ControlFlow<()>, JqError>,
) -> Result<(), JqError> {
    let values = RcIter::new(doc.0.iter().cloned().map(Ok));
    let globals = Globals { lut: &filter.lut, inputs: &values, stop };
    let run = || {
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
    };
    // Nothing the run touched is used after an unwind: the error is the
    // whole run's, as the output cap's is.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|payload| {
        match payload.downcast::<Stopped>() {
            Ok(stopped) => Err(JqError::Stopped { limit: stopped.0 }),
            Err(other) => std::panic::resume_unwind(other),
        }
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
pub(crate) mod tests {
    use super::*;

    const DOC: &str = r#"{"data":{"items":[{"id":1,"name":"a","status":"active"},{"id":2,"name":"b","status":"off"}],"total":2}}"#;

    fn doc() -> JqDocument {
        JqDocument::parse(DOC).expect("fixture is JSON")
    }

    /// A limit no test here reaches unless it means to.
    const LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// [`super::run`] with [`LIMIT`] and no quit flag, for the tests that
    /// don't test either.
    fn run(code: &str, doc: &JqDocument) -> Result<JqRun, JqError> {
        super::run(code, doc, LIMIT, None)
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
        let code = "if . == 3 then [range(1000)] else . end";
        let err = run_with_cap(code, &doc, 1024, LIMIT, None).unwrap_err();
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

    /// #228: what isn't JSON to serde (the Tree tab's reader) isn't JSON to
    /// jq either, though jaq's own reader takes `#` comments and NaN.
    #[test]
    fn comments_and_non_finite_numbers_are_not_json() {
        for body in ["{\"a\":1} # note", "NaN", "nan", "[1, Infinity]", "-Infinity", "1\n# c\n2"] {
            let err = JqDocument::parse(body).unwrap_err();
            assert!(err.message().starts_with("not JSON: "), "{body:?}: {err:?}");
        }
        let doc = JqDocument::parse("data: NaN\n\ndata: 1\n\n").expect("one event is JSON");
        assert_eq!(outputs(".", &doc), vec!["1"], "the NaN event is skipped, as the Tree skips it");
    }

    fn nested(open: &str, close: &str, depth: usize) -> String {
        open.repeat(depth) + "1" + &close.repeat(depth)
    }

    /// #248: jaq's reader recurses once per level and overflowed the stack
    /// (an abort, not a panic) on bodies this deep.
    #[test]
    fn a_body_nested_past_the_limit_is_declined_instead_of_overflowing_the_stack() {
        for body in [nested("[", "]", 100_000), nested("{\"a\":", "}", 100_000)] {
            let err = JqDocument::parse(&body).unwrap_err();
            assert_eq!(err.message(), "the body is nested deeper than 256 levels");
        }
        // One deep line fails the stream, as any bad value does.
        let stream = format!("{{\"a\":1}}\n{}\n", nested("[", "]", 100_000));
        assert!(JqDocument::parse(&stream).is_err());
        // A deep event is skipped, as an event that isn't JSON is.
        let events = format!("data: {}\n\ndata: 2\n\n", nested("[", "]", 100_000));
        assert_eq!(outputs(".", &JqDocument::parse(&events).unwrap()), vec!["2"]);
    }

    /// On this test's thread, a 2 MB stack: a body at the limit is read
    /// and printed, as `.` and by `tojson`, without overflowing it.
    #[test]
    fn a_body_at_the_nesting_limit_is_read_and_printed() {
        for (open, close) in [("[", "]"), ("{\"a\":", "}"), ("[{\"a\":", "}]")] {
            let per = open.matches(['[', '{']).count();
            let at = nested(open, close, MAX_DEPTH / per);
            let doc = JqDocument::parse(&at).expect(open);
            assert_eq!(outputs(".", &doc), vec![at.clone()], "{open}");
            assert_eq!(outputs("tojson | fromjson", &doc), vec![at], "{open}");
            let past = nested(open, close, MAX_DEPTH / per + 1);
            assert!(JqDocument::parse(&past).is_err(), "{open}");
        }
    }

    #[test]
    fn only_brackets_jaq_reads_as_nesting_count_toward_the_limit() {
        let brackets = "[".repeat(500);
        // In a string, after an escaped quote too, and one value after another.
        for body in [
            format!("\"{brackets}\""),
            format!("{{\"a \\\" {brackets}\":\"\\\\\",\"b\":\"{brackets}\"}}"),
            "[[1]]\n".repeat(500),
        ] {
            assert!(JqDocument::parse(&body).is_ok(), "{body:.40}");
        }
        // jaq skips a `#` comment, so a quote in one opens no string that
        // would hide the brackets after it.
        let hidden = format!("# \"\n{}", nested("[", "]", 100_000));
        assert!(JqDocument::parse(&hidden).is_err());
    }

    #[test]
    fn output_past_the_cap_is_refused_rather_than_collected() {
        let err = run_with_cap("[range(100000)]", &doc(), 1024, LIMIT, None).unwrap_err();
        let JqError::Runtime { message } = &err else {
            panic!("expected a runtime error, got {err:?}");
        };
        assert!(message.contains("too large"), "message: {message}");
    }

    /// #310 spec L1, L6, #308 B1: a run that never ends is stopped at its
    /// limit, whatever shape its loop has, and a filter's `try` doesn't
    /// catch it.
    #[test]
    fn a_run_that_never_ends_is_stopped_at_its_limit() {
        let limit = std::time::Duration::from_millis(50);
        let stream = JqDocument::parse(STREAM).unwrap();
        for code in [
            "last(range(1e18))",
            "[range(1e10)]",
            "[repeat(1)]",
            "def f: f; f",
            "until(false; .)",
            "try (last(range(1e18))) catch true",
            "(last(range(1e18)))? // true",
            "reduce range(1e18) as $x (0; . + 1)",
            "label $out | repeat(1) | select(false)",
        ] {
            for doc in [doc(), stream.clone()] {
                let started = std::time::Instant::now();
                let err = super::run(code, &doc, limit, None).unwrap_err();
                assert_eq!(err.message(), "stopped at the 50 ms limit", "{code}");
                assert_eq!(err.stopped_at(), Some(limit), "{code}");
                assert!(started.elapsed() < std::time::Duration::from_secs(5), "{code}");
            }
        }
    }

    /// #310 review round 1: a run whose steps are each slow (a long string
    /// exploded and imploded on every pass, about 30 ms in a debug build)
    /// is stopped near its limit too, not after a fixed number of its steps.
    #[test]
    fn a_run_of_slow_steps_is_stopped_near_its_limit() {
        let limit = std::time::Duration::from_millis(50);
        let started = std::time::Instant::now();
        let code = "\"x\" * 200000 | until(false; explode | implode)";
        let err = super::run(code, &doc(), limit, None).unwrap_err();
        assert_eq!(err.message(), "stopped at the 50 ms limit");
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_millis(1000), "{took:?}");
    }

    /// The outputs of `code` over `doc` compiled with jaq's own natives, its
    /// `range/3` included, for comparing postui's copy with it.
    fn with_jaqs_range(code: &str, doc: &JqDocument) -> JqRun {
        let arena = Arena::default();
        let modules = Loader::new(defs()).load(&arena, File { code, path: () }).unwrap();
        let inputs = jaq_std::input::funs().into_vec().into_iter().map(jaq_core::native::run);
        let funs = jaq_core::funs::<Data>().chain(jaq_std::funs()).chain(inputs);
        let filter =
            Compiler::default().with_funs(funs.chain(jaq_json::funs())).compile(modules).unwrap();
        collect(&filter, doc, OUTPUT_CAP, &Stop::default()).unwrap()
    }

    /// `range`'s copy and the claim that no other native loops without
    /// passing through `lut()` (#310 spec, §1) were checked against these
    /// versions: an update has to check them again.
    #[test]
    fn the_jaq_crates_are_the_versions_range_was_checked_against() {
        let lock = include_str!("../../../../Cargo.lock");
        for (name, version) in [("jaq-core", "3.1.1"), ("jaq-std", "3.0.3"), ("jaq-json", "2.0.3")]
        {
            let entry = format!("name = \"{name}\"\nversion = \"{version}\"\n");
            assert!(lock.contains(&entry), "{name} is no longer {version}");
        }
    }

    /// #310 spec L6: postui's `range` gives what jaq's does.
    #[test]
    fn range_gives_what_jaqs_did() {
        for (code, want) in [
            ("[range(5)]", "[0,1,2,3,4]"),
            ("[range(2; 8)]", "[2,3,4,5,6,7]"),
            ("[range(0; 10; 3)]", "[0,3,6,9]"),
            ("[range(5; 0; -2)]", "[5,3,1]"),
            ("[range(0; 0)]", "[]"),
            ("[range(3; 0)]", "[]"),
            ("[limit(3; range(0; 1; 0))]", "[0,0,0]"),
            ("[range(0.5; 3)]", "[0.5,1.5,2.5]"),
            ("[range(1; 3) as $x | range($x)]", "[0,0,1]"),
            ("[range(0, 1; 3, 4)]", "[0,1,2,0,1,2,3,1,2,1,2,3]"),
        ] {
            assert_eq!(outputs(code, &doc()), [want], "{code}");
            assert_eq!(with_jaqs_range(code, &doc()).outputs, [want], "{code}");
        }
        // A step that can't be added ends the range with its error, after
        // the values before it.
        let (code, one) = (".[] | range(0; 3; \"a\")", JqDocument::parse("[1]").unwrap());
        let jaqs = with_jaqs_range(code, &one);
        assert_eq!(jaqs.outputs, ["0"]);
        assert!(jaqs.error.is_some(), "{jaqs:?}");
        assert_eq!(run(code, &one).unwrap(), jaqs);
    }

    /// #310 spec L6: a filter that ends gives the same run under a limit as
    /// with a stop nothing sets.
    #[test]
    fn a_filter_that_ends_gives_the_same_run_under_a_limit() {
        let stream = JqDocument::parse(STREAM).unwrap();
        for (code, doc) in [
            (".data.items[] | .id", doc()),
            (r#".data.items | map(select(.status == "active")) | map(.name)"#, doc()),
            (".data.total | .x", doc()),
            (".a", stream.clone()),
            ("[., inputs] | map(.a)", stream.clone()),
            ("[limit(5000; repeat(1))] | length", doc()),
        ] {
            let unstopped =
                with_compiled(code, |filter| collect(filter, &doc, OUTPUT_CAP, &Stop::default()));
            assert_eq!(run(code, &doc), unstopped, "{code}");
        }
        assert!(run(".a ==", &doc()).unwrap_err().incomplete());
    }

    /// Sets `quit` 50 ms from now, from another thread, while `run` runs.
    pub(crate) fn quit_soon<T>(quit: &std::sync::atomic::AtomicBool, run: impl FnOnce() -> T) -> T {
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                quit.store(true, std::sync::atomic::Ordering::Relaxed);
            });
            run()
        })
    }

    /// #308 spec B7: a run stops soon after its quit flag is set, long
    /// before its limit.
    #[test]
    fn a_run_stops_once_its_quit_flag_is_set() {
        let quit = std::sync::atomic::AtomicBool::new(false);
        let limit = std::time::Duration::from_secs(10);
        let started = std::time::Instant::now();
        let err = quit_soon(&quit, || {
            super::run("last(range(1e18))", &doc(), limit, Some(&quit)).unwrap_err()
        });
        assert_eq!(err.stopped_at(), Some(limit));
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    }

    /// #308 spec B1: the limit is the one passed in. The filter runs 100
    /// times the short limit or more, so a late timer still stops it.
    #[test]
    fn the_limit_is_the_one_passed_in() {
        let code = "last(range(10000000))";
        let err = super::run(code, &doc(), std::time::Duration::from_millis(1), None).unwrap_err();
        assert_eq!(err.message(), "stopped at the 1 ms limit");
        assert_eq!(outputs(code, &doc()), ["9999999"]);
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
