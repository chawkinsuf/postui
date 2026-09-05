//! Undo/redo history. `History` owns the coalescing logic so callers (`App`)
//! stay dumb: they build a [`Step`] describing what changed and hand it to
//! [`History::record`]; merging bursts of typing into one undo step happens
//! here.

use crate::components::editor::EditorTab;
use postui_core::model::HttpRequest;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone)]
pub enum StepKind {
    EditorDelta {
        slug: Option<String>,
        before: Box<HttpRequest>,
        after: Box<HttpRequest>,
    },
    FileStates {
        before: Vec<(PathBuf, Option<String>)>,
        after: Vec<(PathBuf, Option<String>)>,
        active_env: Option<(Option<String>, Option<String>)>,
        /// What the op did to the request order lists (a create's
        /// arrival, a rename's slot, a move's remove-then-arrive): undo
        /// replays them backwards, redo forwards, so the list ends up
        /// exactly as it was on either side.
        orders: Vec<postui_core::order::OrderEdit>,
        /// `(old, new)` slug pairs for every request the op moved
        /// (rename, move to space, move all) — how undo/redo find where
        /// the open request went, however many files the step covers.
        moves: Vec<(String, String)>,
    },
    /// A delete that went to `.local/trash` (request, environment, or a
    /// whole space). Undo renames the items back (reverse order) and
    /// rewrites `files_before`; redo re-trashes them and rewrites
    /// `files_after`. The companion files are the small ones a delete also
    /// touched (`project.toml`'s `spaces`, `.local/secrets.toml`) — the
    /// trashed payload itself is never held in memory.
    Trashed {
        items: Vec<postui_core::trash::Trashed>,
        files_before: Vec<(PathBuf, Option<String>)>,
        files_after: Vec<(PathBuf, Option<String>)>,
        active_env: Option<(Option<String>, Option<String>)>,
        /// See `FileStates::orders`: the delete's removal from the list.
        orders: Vec<postui_core::order::OrderEdit>,
    },
    /// A reorder (a dropped row drag, alt+↑/↓, a menu's Move up/down): a
    /// list in `project.toml` went from `before` to `after` and nothing
    /// else changed. `burst` marks a keyboard move: quick successive
    /// presses on the same target merge into one step (see
    /// `History::try_merge`); a drag never merges.
    Reorder {
        target: ReorderTarget,
        before: Vec<String>,
        after: Vec<String>,
        burst: bool,
    },
}

/// Which list a `Reorder` step rewrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReorderTarget {
    /// `[space.<space>] order`; `slug` is the request that moved, which
    /// the selection lands back on.
    Requests { space: String, slug: String },
    /// The `spaces` key; `name` is the space that moved, which the Manage
    /// cursor lands back on.
    Spaces { name: String },
}

/// `s`, re-keyed if it names `from` or a request inside it.
fn rekey_slug(s: &mut String, from: &str, to: &str) {
    if s == from {
        *s = to.to_string();
    } else if let Some(rest) = s.strip_prefix(from).and_then(|r| r.strip_prefix('/')) {
        *s = format!("{to}/{rest}");
    }
}

/// `p`, re-keyed if it lies under space `from`'s directory.
fn rekey_path(p: &mut PathBuf, root: &Path, from: &str, to: &str) {
    let old = postui_core::project::space_dir(root, from);
    if let Ok(rest) = p.strip_prefix(&old) {
        let new = postui_core::project::space_dir(root, to);
        *p = if rest.as_os_str().is_empty() {
            new
        } else {
            new.join(rest)
        };
    }
}

impl Step {
    /// Re-keys everything in the step that names space `from` after it
    /// was renamed to `to`: request slugs, request file paths, order
    /// edits and reorder targets. A space rename is not itself an undo
    /// step, so the steps made before it must keep pointing at the space
    /// under its new name or their undo would write to a space that no
    /// longer exists.
    pub fn rename_space(&mut self, root: &Path, from: &str, to: &str) {
        if let Some(slug) = self.context.slug.as_mut() {
            rekey_slug(slug, from, to);
        }
        match &mut self.kind {
            StepKind::EditorDelta { slug, .. } => {
                if let Some(slug) = slug.as_mut() {
                    rekey_slug(slug, from, to);
                }
            }
            StepKind::FileStates {
                before,
                after,
                orders,
                moves,
                ..
            } => {
                for (p, _) in before.iter_mut().chain(after.iter_mut()) {
                    rekey_path(p, root, from, to);
                }
                for e in orders {
                    e.rename_space(from, to);
                }
                for (old, new) in moves {
                    rekey_slug(old, from, to);
                    rekey_slug(new, from, to);
                }
            }
            StepKind::Trashed {
                items,
                files_before,
                files_after,
                orders,
                ..
            } => {
                for t in items {
                    rekey_path(&mut t.original, root, from, to);
                }
                for (p, _) in files_before.iter_mut().chain(files_after.iter_mut()) {
                    rekey_path(p, root, from, to);
                }
                for e in orders {
                    e.rename_space(from, to);
                }
            }
            StepKind::Reorder {
                target,
                before,
                after,
                ..
            } => match target {
                ReorderTarget::Requests { space, slug } => {
                    rekey_slug(space, from, to);
                    rekey_slug(slug, from, to);
                }
                ReorderTarget::Spaces { name } => {
                    rekey_slug(name, from, to);
                    for n in before.iter_mut().chain(after.iter_mut()) {
                        rekey_slug(n, from, to);
                    }
                }
            },
        }
    }
}

/// Where the cursor sat before/after a step, so undo/redo can restore it.
#[derive(Debug, Clone)]
pub struct Context {
    pub slug: Option<String>,
    pub cursor_before: CursorPos,
    pub cursor_after: CursorPos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorPos {
    Url(usize),
    Body { row: usize, col: usize },
    Cell { tab: EditorTab, key: String },
    None,
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
    /// with the same `slug` whose `coalesce_key`s agree (and are `Some`),
    /// or keyboard `Reorder`s of the same request. Merging keeps the top's
    /// `before`/`cursor_before` and takes `step`'s `after`/`cursor_after`;
    /// a reorder burst that ends back where it started is dropped
    /// altogether. Clears the redo stack either way and evicts the oldest
    /// step past the cap.
    pub fn record(&mut self, step: Step, now: Instant) {
        self.redo.clear();

        let merged = self.coalescing
            && self
                .last_record
                .is_some_and(|last| now - last < COALESCE_WINDOW)
            && Self::try_merge(self.undo.last_mut(), &step);

        if !merged {
            self.undo.push(step);
        } else if self.undo.last().is_some_and(Self::is_noop_reorder) {
            // The burst netted to nothing: drop it, and do not let the
            // step it uncovers (however old) absorb the next keystroke.
            self.undo.pop();
            self.last_record = Some(now);
            self.coalescing = false;
            return;
        }

        self.last_record = Some(now);
        self.coalescing = true;

        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// A merged reorder whose list is back where the burst started.
    fn is_noop_reorder(step: &Step) -> bool {
        matches!(&step.kind, StepKind::Reorder { before, after, .. } if before == after)
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

    /// Re-keys every step on both stacks after space `from` was renamed
    /// `to` (see `Step::rename_space`).
    pub fn rename_space(&mut self, root: &Path, from: &str, to: &str) {
        for step in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            step.rename_space(root, from, to);
        }
    }

    /// Attempts to merge `new` into `top` in place. Returns whether it did.
    fn try_merge(top: Option<&mut Step>, new: &Step) -> bool {
        let Some(top) = top else { return false };
        if let (
            StepKind::Reorder {
                target: top_target,
                after,
                burst: true,
                ..
            },
            StepKind::Reorder {
                target: new_target,
                after: new_after,
                burst: true,
                ..
            },
        ) = (&mut top.kind, &new.kind)
        {
            // Two keyboard moves of the same target: the burst is one
            // rewrite from the first `before` to the last `after`.
            if top_target != new_target {
                return false;
            }
            *after = new_after.clone();
            return true;
        }
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
        top.context.cursor_after = new.context.cursor_after.clone();
        true
    }

    /// The top undo step without popping it. Test-only: production code
    /// drives undo/redo through `pop_undo`/`pop_redo`.
    #[cfg(test)]
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

    fn reorder(before: &[&str], after: &[&str], burst: bool) -> Step {
        Step {
            kind: StepKind::Reorder {
                target: ReorderTarget::Requests {
                    space: "main".into(),
                    slug: "main/a".into(),
                },
                before: before.iter().map(|s| s.to_string()).collect(),
                after: after.iter().map(|s| s.to_string()).collect(),
                burst,
            },
            context: Context {
                slug: None,
                cursor_before: CursorPos::None,
                cursor_after: CursorPos::None,
            },
        }
    }

    #[test]
    fn a_burst_that_nets_to_nothing_is_dropped_and_does_not_reopen_the_step_beneath() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(reorder(&["a", "b"], &["b", "a"], true), t0);
        assert_eq!(h.undo_len(), 1);
        // A minute later: a fresh burst that goes down then straight back.
        let t1 = t0 + Duration::from_secs(60);
        h.record(reorder(&["b", "a"], &["a", "b"], true), t1);
        assert_eq!(h.undo_len(), 2);
        h.record(
            reorder(&["a", "b"], &["b", "a"], true),
            t1 + Duration::from_millis(500),
        );
        assert_eq!(h.undo_len(), 1, "down then up is dropped");
        // The next move must be its own step, not a merge into the
        // minute-old step the pop uncovered.
        h.record(
            reorder(&["b", "a"], &["a", "b"], true),
            t1 + Duration::from_secs(1),
        );
        assert_eq!(h.undo_len(), 2);
        let Some(Step {
            kind: StepKind::Reorder { before, .. },
            ..
        }) = h.peek_undo()
        else {
            panic!("reorder on top")
        };
        assert_eq!(before, &["b", "a"]);
    }

    #[test]
    fn a_drag_reorder_never_merges() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(reorder(&["a", "b"], &["b", "a"], true), t0);
        h.record_no_coalesce(reorder(&["b", "a"], &["a", "b"], false));
        h.record(
            reorder(&["a", "b"], &["b", "a"], true),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(h.undo_len(), 3);
    }
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
                cursor_before: CursorPos::Url(before.len()),
                cursor_after: CursorPos::Url(after.len()),
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
                    cursor_before: CursorPos::None,
                    cursor_after: CursorPos::None,
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
    fn merged_step_keeps_before_cursor_takes_after_cursor() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(delta("ab", "abc"), t0);
        h.record(delta("abc", "abcd"), t0 + Duration::from_millis(100));
        let step = h.pop_undo().unwrap();
        assert_eq!(step.context.cursor_before, CursorPos::Url(2));
        assert_eq!(step.context.cursor_after, CursorPos::Url(4));
    }
}
