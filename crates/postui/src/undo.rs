//! Undo/redo history. `History` owns the coalescing logic so callers (`App`)
//! stay dumb: they build a [`Step`] describing what changed and hand it to
//! [`History::record`]; merging bursts of typing into one undo step happens
//! here.

use crate::components::sidebar::RowKey;
use postui_core::model::HttpRequest;
use std::time::{Duration, Instant};

/// Steps older than this are evicted once the undo stack exceeds it.
const MAX_STEPS: usize = 200;
/// Consecutive edits within this window (and with a matching
/// [`coalesce_key`]) merge into a single undo step.
const COALESCE_WINDOW: Duration = Duration::from_secs(2);

/// One undoable change.
#[derive(Debug, Clone)]
pub struct Step {
    pub kind: StepKind,
    pub context: Context,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiValue {
    Flag(bool),
    Text(String),
    Int(usize),
}

#[derive(Debug, Clone)]
pub enum StepKind {
    EditorDelta {
        slug: Option<String>,
        before: Box<HttpRequest>,
        after: Box<HttpRequest>,
    },
    /// One `Project` journal entry: undoing this step calls
    /// `Project::undo`, whose own journal holds the inverse ops. `id` is
    /// checked against the journal's top before replay: a marker whose
    /// entry was merged into an earlier one, netted to nothing, or evicted
    /// by the journal cap is skipped silently.
    Project {
        id: postui_core::journal::EntryId,
        /// The undo/redo toast's wording, captured when the op ran (spec
        /// R2).
        label: StepLabel,
        /// The view just before the op and just after it (spec §4.1):
        /// undo lands on the parts that differ, taken from `before`; redo
        /// from `after`.
        before: Box<View>,
        after: Box<View>,
    },
    /// One Settings-tab write (`Action::SetUiFlag` / `SetUiString` /
    /// `SetUiInt`), spec 2026-09-16: undoing writes `before` back through
    /// `Config::save_ui_*` and reapplies it; redoing writes `after`. Never
    /// coalesced — each commit (a field close, a toggle, a segment flip)
    /// is its own step.
    Config {
        key: &'static str,
        before: UiValue,
        after: UiValue,
    },
    /// A `config.toml` / `keys.toml` Reset (`Action::ForceResetConfigFile`).
    /// `before` is the file's bytes before the reset (`None` when the file
    /// did not exist); undo writes `before` back (or removes the file) and
    /// reloads, redo writes `after`.
    ConfigFile {
        file: crate::action::ConfigFile,
        before: Option<String>,
        after: String,
    },
}

/// Which request a step belongs to, so undo can jump back to it. No caret
/// is stored: undo/redo leave focus and the caret exactly where they are
/// (ruling 2026-09-18, matching the Manage screen) — `Editor::apply_snapshot`
/// re-places the caret it already has against the swapped-in fields.
#[derive(Debug, Clone)]
pub struct Context {
    pub slug: Option<String>,
}

/// The part of the app's view an undo can put back (spec §4.1): the
/// active space, what the editor holds (with its unsaved buffer when it
/// has one), the sidebar cursor, and the Manage/Variables row. A project
/// step stores one from just before its op and one from just after.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub space: String,
    pub open: Open,
    pub cursor: Option<RowKey>,
    pub row: Option<ListRow>,
}

/// What the editor holds. `buffer` is `Some` only while the editor holds
/// work a switch would lose (`App::editor_holds_unsaved`).
#[derive(Debug, Clone, PartialEq)]
pub enum Open {
    Request { slug: String, buffer: Option<Box<HttpRequest>> },
    Scratch { buffer: Option<Box<HttpRequest>> },
}

impl Open {
    /// Whether two views hold the same thing to land on: the same request
    /// (or both a scratch), with a buffer on both or on neither (§4.4).
    pub fn same_target(&self, other: &Open) -> bool {
        match (self, other) {
            (Open::Request { slug: a, buffer: x }, Open::Request { slug: b, buffer: y }) => {
                a == b && x.is_some() == y.is_some()
            }
            (Open::Scratch { buffer: x }, Open::Scratch { buffer: y }) => x.is_some() == y.is_some(),
            _ => false,
        }
    }

    /// The request's slug; `None` for a scratch.
    pub fn slug(&self) -> Option<&str> {
        match self {
            Open::Request { slug, .. } => Some(slug),
            Open::Scratch { .. } => None,
        }
    }
}

/// A Manage-screen row by identity: which list, which name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListRow {
    Env(String),
    Space(String),
    Var(String),
}

/// What a project step did, for its undo/redo toast (spec R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Create,
    Rename,
    Move { to: String },
    Reorder,
    Change,
    Delete,
    ProjectChange,
}

/// A project step's toast wording, captured when the op ran so it never
/// names a request that merely happened to be open, nor reads a name
/// after the file is gone (spec R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLabel {
    pub verb: Verb,
    pub subject: String,
}

impl StepLabel {
    fn of(verb: Verb, subject: impl Into<String>) -> Self {
        Self { verb, subject: subject.into() }
    }
    pub fn create(display: impl Into<String>) -> Self { Self::of(Verb::Create, display) }
    pub fn rename(new_display: impl Into<String>) -> Self { Self::of(Verb::Rename, new_display) }
    pub fn moved(display: impl Into<String>, space: impl Into<String>) -> Self {
        Self::of(Verb::Move { to: space.into() }, display)
    }
    pub fn moved_all(n: usize, space: impl Into<String>) -> Self {
        let noun = if n == 1 { "request" } else { "requests" };
        Self::of(Verb::Move { to: space.into() }, format!("{n} {noun}"))
    }
    pub fn reorder(what: impl Into<String>) -> Self { Self::of(Verb::Reorder, what) }
    pub fn change(what: impl Into<String>) -> Self { Self::of(Verb::Change, what) }
    pub fn variable(name: &str) -> Self { Self::change(format!("variable {name}")) }
    pub fn environment(display: &str) -> Self { Self::change(format!("environment {display}")) }
    pub fn space(display: &str) -> Self { Self::change(format!("space {display}")) }
    pub fn project() -> Self { Self::of(Verb::ProjectChange, "") }
    pub fn delete(what: impl Into<String>) -> Self { Self::of(Verb::Delete, what) }
    pub fn delete_env(display: &str) -> Self { Self::delete(format!("environment {display}")) }
    pub fn delete_space(display: &str) -> Self { Self::delete(format!("space {display}")) }
    pub fn delete_variable(name: &str) -> Self { Self::delete(format!("\"{name}\"")) }
    pub fn delete_option(name: &str, env: &str) -> Self {
        Self::delete(format!("option \"{name}\" in {env}"))
    }

    /// The toast an undo (`redo == false`) or redo of this step shows.
    pub fn toast(&self, redo: bool) -> String {
        let done = if redo { "Redid" } else { "Undid" };
        let s = &self.subject;
        match &self.verb {
            Verb::Delete if redo => format!("Deleted {s} again"),
            Verb::Delete => format!("Restored {s}"),
            Verb::Create => format!("{done} create of {s}"),
            Verb::Rename => format!("{done} rename to {s}"),
            Verb::Move { to } => format!("{done} move of {s} to {to}"),
            Verb::Reorder => format!("{done} reorder of {s}"),
            Verb::Change => format!("{done} change to {s}"),
            Verb::ProjectChange => format!("{done} project change"),
        }
    }
}

/// Which single `HttpRequest` field changed, for burst-coalescing purposes.
/// `None` means either nothing or more than one field differs — such steps
/// never merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoalesceKey {
    Url,
    Body,
    Name,
    Jq,
}

/// `Some(key)` when exactly one of `url`/`body`/`name`/`jq` differs between
/// `before` and `after`; `None` otherwise (method, table maps,
/// `substitute_body`, or multiple fields at once — those never coalesce).
pub fn coalesce_key(before: &HttpRequest, after: &HttpRequest) -> Option<CoalesceKey> {
    let url = before.url != after.url;
    let body = before.body != after.body;
    let name = before.name != after.name;
    let jq = before.jq != after.jq;
    let other = before.method != after.method
        || before.substitute_body != after.substitute_body
        || before.insecure != after.insecure
        || before.params != after.params
        || before.headers != after.headers
        || before.variables != after.variables;
    match (url, body, name, jq, other) {
        (true, false, false, false, false) => Some(CoalesceKey::Url),
        (false, true, false, false, false) => Some(CoalesceKey::Body),
        (false, false, true, false, false) => Some(CoalesceKey::Name),
        (false, false, false, true, false) => Some(CoalesceKey::Jq),
        _ => None,
    }
}

/// Undo/redo stacks with typing-burst coalescing and a 200-step cap.
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_record: Option<Instant>,
    /// Whether the next matching `EditorDelta` may merge into the top undo
    /// step. Cleared by `pop_undo`/`pop_redo`/`push_undo_no_coalesce`/
    /// `break_coalescing`; set by `record`.
    coalescing: bool,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last_record: None,
            coalescing: false,
        }
    }

    /// Records `step`, merging it into the top undo step when all of these
    /// hold: coalescing is active, `now` is within the coalesce window of
    /// the last record, and the two steps are mergeable — `EditorDelta`s
    /// with the same `slug` whose `coalesce_key`s agree (and are `Some`).
    /// Merging keeps the top's `before` and takes `step`'s `after`. Clears
    /// the redo stack either way and evicts
    /// the oldest step past the cap.
    pub fn record(&mut self, step: Step, now: Instant) {
        self.redo.clear();

        let merged = self.coalescing
            && self
                .last_record
                .is_some_and(|last| now - last < COALESCE_WINDOW)
            && Self::try_merge(self.undo.last_mut(), &step);

        if !merged {
            self.undo.push(step);
        }

        self.last_record = Some(now);
        self.coalescing = true;

        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// `record` or `record_no_coalesce`, by flag: the one place that
    /// decides how a step joins the stack.
    pub fn record_maybe_coalesce(&mut self, step: Step, coalesce: bool) {
        if coalesce {
            self.record(step, Instant::now());
        } else {
            self.record_no_coalesce(step);
        }
    }

    /// Attempts to merge `new` into `top` in place. Returns whether it did.
    fn try_merge(top: Option<&mut Step>, new: &Step) -> bool {
        let Some(top) = top else { return false };
        let StepKind::EditorDelta {
            slug: top_slug,
            before: top_before,
            after: top_after,
        } = &mut top.kind
        else {
            return false;
        };
        let StepKind::EditorDelta {
            slug: new_slug,
            before: new_before,
            after: new_after,
        } = &new.kind
        else {
            return false;
        };
        if top_slug != new_slug {
            return false;
        }
        let top_key = coalesce_key(top_before, top_after);
        let new_key = coalesce_key(new_before, new_after);
        if top_key.is_none() || top_key != new_key {
            return false;
        }

        *top_after = new_after.clone();
        true
    }

    /// The top undo step without popping it. Undo/redo itself goes
    /// through `pop_undo`/`pop_redo`; this is for the one caller that has
    /// to know *what* is on top before deciding to drop it (a `Project`
    /// marker whose journal entry a burst merge dissolved).
    pub fn peek_undo(&self) -> Option<&Step> {
        self.undo.last()
    }

    /// Pops the most recent undo step, if any, breaking coalescing.
    pub fn pop_undo(&mut self) -> Option<Step> {
        self.coalescing = false;
        self.undo.pop()
    }

    /// Pops the most recent redo step, if any, breaking coalescing.
    pub fn pop_redo(&mut self) -> Option<Step> {
        self.coalescing = false;
        self.redo.pop()
    }

    /// Pushes `step` onto the redo stack (the Undo arm's counterpart to
    /// popping an undo step).
    pub fn push_redo(&mut self, step: Step) {
        self.redo.push(step);
    }

    /// Records `step` on the undo stack without merging, and leaves
    /// coalescing off so nothing merges into it later. Used for wholesale
    /// changes (format/minify, discard, method change, insert-var, `$EDITOR`
    /// round-trip) and every `FileStates` step — also the Redo arm's way
    /// of pushing a step back onto undo.
    pub fn push_undo_no_coalesce(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.coalescing = false;
    }

    /// Records `step` as a fresh, non-coalescing undo step and clears the
    /// redo stack — unlike `push_undo_no_coalesce`, which deliberately
    /// leaves redo alone for undo/redo apply push-backs. Used when a new
    /// (not replayed) step must not merge with what came before *and* must
    /// invalidate any stale redo entries (spec's linear-history rule).
    pub fn record_no_coalesce(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.last_record = Some(Instant::now());
        self.coalescing = false;
    }

    /// Clears both stacks and coalescing state.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_record = None;
        self.coalescing = false;
    }

    /// Stops the next `record` from merging into the current top step.
    /// Called after undo/redo so the next edit starts a fresh step.
    pub fn break_coalescing(&mut self) {
        self.coalescing = false;
    }

    /// Number of steps on the undo stack. Test-only: production code
    /// drives undo/redo through `pop_undo`/`pop_redo`, never by counting.
    #[cfg(test)]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Number of steps on the redo stack. Test-only, see `undo_len`.
    #[cfg(test)]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use postui_core::model::{HttpRequest, Method};
    use std::time::{Duration, Instant};

    fn req(url: &str) -> HttpRequest {
        HttpRequest {
            name: None,
            method: Method::Get,
            url: url.into(),
            substitute_body: false,
            insecure: false,
            jq: None,
            jq_enabled: true,
            params: Default::default(),
            headers: Default::default(),
            variables: Default::default(),
            body: None,
        }
    }

    fn delta(before: &str, after: &str) -> Step {
        Step {
            kind: StepKind::EditorDelta {
                slug: Some("a".into()),
                before: Box::new(req(before)),
                after: Box::new(req(after)),
            },
            context: Context {
                slug: Some("a".into()),
            },
        }
    }

    #[test]
    fn typing_burst_coalesces_into_one_step() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        h.record(delta("h", "ht"), t0 + Duration::from_millis(100));
        h.record(delta("ht", "htt"), t0 + Duration::from_millis(200));
        let step = h.pop_undo().unwrap();
        assert!(h.pop_undo().is_none(), "burst must be one step");
        let StepKind::EditorDelta { before, after, .. } = step.kind else {
            panic!()
        };
        assert_eq!(before.url, "");
        assert_eq!(after.url, "htt");
    }

    #[test]
    fn pause_breaks_the_burst() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        h.record(delta("h", "ht"), t0 + Duration::from_secs(3));
        assert!(h.pop_undo().is_some());
        assert!(h.pop_undo().is_some(), "pause must split into two steps");
    }

    #[test]
    fn field_switch_breaks_the_burst() {
        // url edit then name edit: different coalesce keys
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        let mut named = req("h");
        named.name = Some("x".into());
        h.record(
            Step {
                kind: StepKind::EditorDelta {
                    slug: Some("a".into()),
                    before: Box::new(req("h")),
                    after: Box::new(named),
                },
                context: Context {
                    slug: Some("a".into()),
                },
            },
            t0 + Duration::from_millis(100),
        );
        assert!(h.pop_undo().is_some());
        assert!(h.pop_undo().is_some());
    }

    #[test]
    fn new_record_clears_redo() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        let s = h.pop_undo().unwrap();
        h.push_redo(s);
        h.record(delta("", "x"), t0 + Duration::from_secs(5));
        assert!(h.pop_redo().is_none());
    }

    #[test]
    fn record_no_coalesce_clears_redo() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        let s = h.pop_undo().unwrap();
        h.push_redo(s);
        h.record_no_coalesce(delta("", "x"));
        assert!(h.pop_redo().is_none());
    }

    #[test]
    fn a_config_step_round_trips_through_a_history_record() {
        let mut h = History::new();
        h.record_no_coalesce(Step {
            kind: StepKind::Config {
                key: "hover_hints",
                before: UiValue::Flag(true),
                after: UiValue::Flag(false),
            },
            context: Context {
                slug: None,
            },
        });
        let popped = h.pop_undo().expect("the step is there");
        assert!(matches!(
            popped.kind,
            StepKind::Config {
                key: "hover_hints",
                before: UiValue::Flag(true),
                after: UiValue::Flag(false),
            }
        ));
    }

    #[test]
    fn undo_then_typing_starts_fresh_step() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("", "h"), t0);
        let s = h.pop_undo().unwrap();
        h.push_redo(s);
        h.break_coalescing();
        h.record(delta("", "z"), t0 + Duration::from_millis(50));
        let StepKind::EditorDelta { before, after, .. } = h.pop_undo().unwrap().kind else {
            panic!()
        };
        assert_eq!((before.url.as_str(), after.url.as_str()), ("", "z"));
    }

    #[test]
    fn cap_evicts_oldest() {
        let mut h = History::new();
        let t0 = Instant::now();
        for i in 0..205 {
            // distinct slugs so nothing coalesces
            let mut s = delta("", "x");
            if let StepKind::EditorDelta { slug, .. } = &mut s.kind {
                *slug = Some(format!("r{i}"));
            }
            h.record(s, t0 + Duration::from_secs(i));
        }
        let mut n = 0;
        while h.pop_undo().is_some() {
            n += 1;
        }
        assert_eq!(n, 200);
    }

    #[test]
    fn coalesce_key_none_when_multiple_fields_differ() {
        let mut b = req("a");
        b.name = Some("n".into());
        assert_eq!(coalesce_key(&req("z"), &b), None);
        assert_eq!(coalesce_key(&req("a"), &req("ab")), Some(CoalesceKey::Url));
    }

    #[test]
    fn coalesce_key_none_when_insecure_flips_alongside_a_url_edit() {
        let mut b = req("ab");
        b.insecure = true;
        assert_eq!(coalesce_key(&req("a"), &b), None);
    }

    #[test]
    fn a_lone_jq_edit_coalesces_under_its_own_key() {
        let before = req("a");
        let mut after = before.clone();
        after.jq = Some(".a".into());
        assert_eq!(coalesce_key(&before, &after), Some(CoalesceKey::Jq));
        after.url.push('x');
        assert_eq!(
            coalesce_key(&before, &after),
            None,
            "two fields at once never coalesce"
        );
    }

    #[test]
    fn step_labels_toast_in_the_spec_wording() {
        let cases: &[(StepLabel, &str, &str)] = &[
            (StepLabel::create("Fresh"), "Undid create of Fresh", "Redid create of Fresh"),
            (StepLabel::rename("Pong"), "Undid rename to Pong", "Redid rename to Pong"),
            (StepLabel::moved("Ping", "Auth"), "Undid move of Ping to Auth", "Redid move of Ping to Auth"),
            (StepLabel::moved_all(3, "Auth"), "Undid move of 3 requests to Auth", "Redid move of 3 requests to Auth"),
            (StepLabel::moved_all(1, "Auth"), "Undid move of 1 request to Auth", "Redid move of 1 request to Auth"),
            (StepLabel::reorder("space Billing"), "Undid reorder of space Billing", "Redid reorder of space Billing"),
            (StepLabel::change("Ping"), "Undid change to Ping", "Redid change to Ping"),
            (StepLabel::variable("base_url"), "Undid change to variable base_url", "Redid change to variable base_url"),
            (StepLabel::environment("Staging"), "Undid change to environment Staging", "Redid change to environment Staging"),
            (StepLabel::space("Auth v2!"), "Undid change to space Auth v2!", "Redid change to space Auth v2!"),
            (StepLabel::project(), "Undid project change", "Redid project change"),
            (StepLabel::delete("Fancy Name!"), "Restored Fancy Name!", "Deleted Fancy Name! again"),
            (StepLabel::delete_env("Staging One"), "Restored environment Staging One", "Deleted environment Staging One again"),
            (StepLabel::delete_space("Auth v2!"), "Restored space Auth v2!", "Deleted space Auth v2! again"),
            (StepLabel::delete_variable("api_key"), "Restored \"api_key\"", "Deleted \"api_key\" again"),
            (StepLabel::delete_option("alice", "QA"), "Restored option \"alice\" in QA", "Deleted option \"alice\" in QA again"),
        ];
        for (label, undo, redo) in cases {
            assert_eq!(label.toast(false), *undo);
            assert_eq!(label.toast(true), *redo);
        }
    }

    #[test]
    fn open_targets_compare_the_slug_and_whether_a_buffer_rides_along() {
        let r = |slug: &str, buf: bool| Open::Request {
            slug: slug.into(),
            buffer: buf.then(|| Box::new(req("x"))),
        };
        assert!(r("a", false).same_target(&r("a", false)));
        assert!(!r("a", false).same_target(&r("b", false)));
        assert!(!r("a", false).same_target(&r("a", true)), "a buffer is part of the target");
        assert!(Open::Scratch { buffer: None }.same_target(&Open::Scratch { buffer: None }));
        assert!(!Open::Scratch { buffer: None }.same_target(&r("a", false)));
    }
}
