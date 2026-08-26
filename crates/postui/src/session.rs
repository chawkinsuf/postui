//! The request session: which request's response is on screen, the
//! per-request cache of earlier responses, and the in-flight sends — at
//! most one per request, any number across requests.
//!
//! A response is not app-global — it is *the response to one request*. This
//! module owns that binding: the on-screen [`Response`] always belongs to
//! the open request, every other request's latest response waits in a
//! session-lifetime cache, and a result that arrives after the user has
//! navigated away lands in its own request's cache slot instead of on
//! screen.

use crate::components::response::{Response, ResponseState};
use std::collections::HashMap;
use std::time::Instant;

/// A dispatched request: when it started (for the elapsed display), which
/// generation it belongs to (so a stale result can be told apart from the
/// current one), which request issued it (so the result lands with its
/// owner), and the task itself (so it can be aborted on cancel).
pub struct InFlight {
    pub started: Instant,
    pub generation: u64,
    /// `editor.slug` at send time; `None` for an unsaved scratch request.
    pub slug: Option<String>,
    pub task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
pub struct Session {
    /// The response shown in the response pane — always the one belonging
    /// to `open_slug`.
    pub response: Response,
    /// Which request `response` belongs to (`None`: unsaved scratch).
    open_slug: Option<String>,
    /// Latest response of every request navigated away from, keyed like
    /// `open_slug`. Session-lifetime only; never persisted.
    cache: HashMap<Option<String>, Response>,
    /// Every send still waiting on its result — at most one entry per
    /// request (`Action::Send` is disabled while its request is in
    /// flight, and `begin_send` defensively supersedes a same-request
    /// entry), any number of entries across requests.
    pub in_flight: Vec<InFlight>,
    /// Bumped on every `begin_send`; tags each spawned send so a result
    /// that arrives after a newer send has started can be told apart and
    /// dropped. Public so tests can fabricate delivery actions.
    pub send_generation: u64,
}

impl Session {
    /// Follows a change of open request: stashes the on-screen response
    /// under the outgoing request and swaps in the incoming request's
    /// cached response, or an empty one. Returns whether anything changed
    /// (i.e. the caller must redraw).
    pub fn sync_open(&mut self, open: &Option<String>) -> bool {
        if &self.open_slug == open {
            return false;
        }
        let outgoing = std::mem::take(&mut self.response);
        // An Empty response carries nothing worth restoring; keeping the
        // cache to requests that actually have a result bounds its size to
        // the requests used this session.
        if !matches!(outgoing.state(), ResponseState::Empty) {
            self.cache.insert(self.open_slug.clone(), outgoing);
        }
        self.response = self.cache.remove(open).unwrap_or_default();
        self.open_slug = open.clone();
        true
    }

    /// The response owned by `slug`: the on-screen one while that request
    /// is open, its cache slot otherwise.
    fn response_for(&mut self, slug: &Option<String>) -> &mut Response {
        if &self.open_slug == slug {
            &mut self.response
        } else {
            self.cache.entry(slug.clone()).or_default()
        }
    }

    /// The in-flight send belonging to `slug`, if any.
    pub fn in_flight_for(&self, slug: &Option<String>) -> Option<&InFlight> {
        self.in_flight.iter().find(|f| &f.slug == slug)
    }

    /// Whether `slug` has a send still waiting — the gate that keeps a
    /// request from being sent twice at once (other requests may send
    /// freely alongside it).
    pub fn is_in_flight(&self, slug: &Option<String>) -> bool {
        self.in_flight_for(slug).is_some()
    }

    /// Starts a new send for `slug` (always the open request): aborts and
    /// drops any previous send *of that request* — other requests' sends
    /// keep running — bumps the generation, and puts the request's
    /// response into `InFlight`. Returns the new generation for the
    /// caller to tag the spawned task's result with; the caller then
    /// tracks the task by pushing onto `in_flight`.
    pub fn begin_send(&mut self, slug: &Option<String>) -> u64 {
        // `Action::Send` is a no-op while `slug` is in flight, so this is
        // defensive: a superseded same-request send is aborted and its
        // entry dropped (its slot is overwritten with the fresh InFlight
        // state below, so there is no Cancelled flash to paint).
        if let Some(i) = self.in_flight.iter().position(|f| &f.slug == slug) {
            self.in_flight.remove(i).task.abort();
        }
        self.send_generation += 1;
        let generation = self.send_generation;
        self.response_for(slug).set_state(
            ResponseState::InFlight {
                started: Instant::now(),
            },
            generation,
        );
        generation
    }

    /// Cancels the *open* request's in-flight send, if any, marking its
    /// response `Cancelled`. Other requests' sends keep running — cancel
    /// is per-request, reached from the request it belongs to.
    pub fn cancel(&mut self) -> bool {
        let Some(i) = self.in_flight.iter().position(|f| f.slug == self.open_slug) else {
            return false;
        };
        let inflight = self.in_flight.remove(i);
        inflight.task.abort();
        // Removing the entry is also what drops a raced result: the task
        // may have already queued its result before the abort landed, and
        // `deliver` only accepts generations it can still find here.
        self.send_generation += 1;
        let generation = self.send_generation;
        self.response_for(&inflight.slug)
            .set_state(ResponseState::Cancelled, generation);
        true
    }

    /// Delivers a completed response to its owner — on screen when that
    /// request is still open, its cache slot otherwise. A result from a
    /// superseded generation is dropped.
    pub fn arrived(&mut self, generation: u64, data: Box<crate::http::ResponseData>) -> bool {
        self.deliver(generation, ResponseState::Ready(data))
    }

    /// Like `arrived`, for a send that failed.
    pub fn failed(&mut self, generation: u64, error: String) -> bool {
        self.deliver(generation, ResponseState::Failed(error))
    }

    fn deliver(&mut self, generation: u64, state: ResponseState) -> bool {
        // A result is current exactly while its send is still tracked:
        // cancel and same-request supersession both remove the entry, so
        // a raced or stale result finds nothing and is dropped.
        let Some(i) = self
            .in_flight
            .iter()
            .position(|f| f.generation == generation)
        else {
            return false;
        };
        let slug = self.in_flight.remove(i).slug;
        self.response_for(&slug).set_state(state, generation);
        true
    }

    /// Delivers a finished background pretty-print to whichever response is
    /// still waiting on it: the one on screen, or — when the user has
    /// navigated away since the send — the cache slot it moved into. `false`
    /// when nothing is waiting (superseded, or the response was replaced).
    pub fn tree_arrived(
        &mut self,
        generation: u64,
        tree: Option<crate::components::json_tree::JsonTree>,
    ) -> bool {
        if self.response.awaits_tree(generation) {
            return self.response.attach_tree(generation, tree);
        }
        for cached in self.cache.values_mut() {
            if cached.awaits_tree(generation) {
                return cached.attach_tree(generation, tree);
            }
        }
        false
    }

    /// Forgets everything — cache, screen, in-flight — for a project
    /// switch: slugs are project-relative, so a cached response would
    /// otherwise leak across projects under a colliding slug.
    pub fn reset(&mut self) {
        for inflight in self.in_flight.drain(..) {
            inflight.task.abort();
        }
        self.send_generation += 1;
        self.cache.clear();
        self.response = Response::default();
        self.open_slug = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn data(body: &str) -> Box<crate::http::ResponseData> {
        Box::new(crate::http::ResponseData {
            status: 200,
            headers: vec![],
            body: body.to_string(),
            elapsed: Duration::from_millis(1),
            size: body.len(),
            content_type: None,
        })
    }

    fn body_of(r: &Response) -> Option<&str> {
        match r.state() {
            ResponseState::Ready(d) => Some(&d.body),
            _ => None,
        }
    }

    fn open(s: &mut Session, slug: &str) {
        s.sync_open(&Some(slug.to_string()));
    }

    async fn in_flight(generation: u64, slug: &str) -> InFlight {
        InFlight {
            started: Instant::now(),
            generation,
            slug: Some(slug.to_string()),
            task: tokio::spawn(async {}),
        }
    }

    #[test]
    fn switching_open_request_swaps_to_cached_or_empty_response() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response
            .set_state(ResponseState::Ready(data("from a")), 0);

        open(&mut s, "b");
        assert!(
            matches!(s.response.state(), ResponseState::Empty),
            "a request never sent shows an empty response, not a's leftovers"
        );
        s.response
            .set_state(ResponseState::Ready(data("from b")), 0);

        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("from a"));
        open(&mut s, "b");
        assert_eq!(body_of(&s.response), Some("from b"));
    }

    #[test]
    fn sync_open_reports_change_only_when_the_request_differs() {
        let mut s = Session::default();
        assert!(s.sync_open(&Some("a".into())));
        assert!(!s.sync_open(&Some("a".into())), "same request: no redraw");
    }

    #[tokio::test]
    async fn result_arriving_after_navigating_away_lands_in_its_requests_cache() {
        let mut s = Session::default();
        open(&mut s, "a");
        let generation = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(generation, "a").await);

        open(&mut s, "b");
        assert!(s.arrived(generation, data("late result")));
        assert!(
            matches!(s.response.state(), ResponseState::Empty),
            "b's screen must not show a's late result"
        );

        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("late result"));
    }

    #[tokio::test]
    async fn result_from_a_superseded_generation_is_dropped() {
        let mut s = Session::default();
        open(&mut s, "a");
        let stale = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(stale, "a").await);
        let current = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(current, "a").await);

        assert!(!s.arrived(stale, data("stale")));
        assert!(
            matches!(s.response.state(), ResponseState::InFlight { .. }),
            "the newer send is still pending"
        );
        assert!(s.arrived(current, data("fresh")));
        assert_eq!(body_of(&s.response), Some("fresh"));
    }

    #[tokio::test]
    async fn sends_from_different_requests_run_concurrently() {
        let mut s = Session::default();
        open(&mut s, "a");
        let first = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(first, "a").await);

        open(&mut s, "b");
        let second = s.begin_send(&Some("b".into()));
        s.in_flight.push(in_flight(second, "b").await);
        assert!(
            s.is_in_flight(&Some("a".into())),
            "b's send leaves a's running"
        );
        assert!(s.is_in_flight(&Some("b".into())));

        assert!(s.arrived(first, data("from a")), "a's result still lands");
        assert!(s.arrived(second, data("from b")));
        assert_eq!(body_of(&s.response), Some("from b"));
        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("from a"));
    }

    #[tokio::test]
    async fn cancel_only_cancels_the_open_requests_send() {
        let mut s = Session::default();
        open(&mut s, "a");
        let first = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(first, "a").await);

        open(&mut s, "b");
        assert!(!s.cancel(), "b has nothing in flight to cancel");
        assert!(s.is_in_flight(&Some("a".into())), "a's send keeps running");

        let second = s.begin_send(&Some("b".into()));
        s.in_flight.push(in_flight(second, "b").await);
        assert!(s.cancel());
        assert!(matches!(s.response.state(), ResponseState::Cancelled));
        assert!(s.is_in_flight(&Some("a".into())), "cancel is per-request");

        assert!(s.arrived(first, data("from a")));
        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("from a"));
    }

    #[tokio::test]
    async fn a_result_racing_a_cancel_is_dropped() {
        let mut s = Session::default();
        open(&mut s, "a");
        let generation = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(generation, "a").await);
        assert!(s.cancel());

        assert!(
            !s.arrived(generation, data("raced past the abort")),
            "a cancelled send's late result must not overwrite Cancelled"
        );
        assert!(matches!(s.response.state(), ResponseState::Cancelled));
    }

    #[tokio::test]
    async fn failure_lands_with_its_owner_too() {
        let mut s = Session::default();
        open(&mut s, "a");
        let generation = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(generation, "a").await);

        open(&mut s, "b");
        assert!(s.failed(generation, "boom".into()));
        assert!(matches!(s.response.state(), ResponseState::Empty));
        open(&mut s, "a");
        assert!(matches!(s.response.state(), ResponseState::Failed(e) if e == "boom"));
    }

    #[tokio::test]
    async fn reset_aborts_every_in_flight_send() {
        let mut s = Session::default();
        open(&mut s, "a");
        let first = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(first, "a").await);
        open(&mut s, "b");
        let second = s.begin_send(&Some("b".into()));
        s.in_flight.push(in_flight(second, "b").await);

        s.reset();
        assert!(s.in_flight.is_empty());
        assert!(!s.arrived(first, data("late")));
        assert!(!s.arrived(second, data("late")));
    }

    #[tokio::test]
    async fn a_background_parse_lands_in_its_requests_cache_slot() {
        let big = format!(
            "{{\"a\": \"{}\"}}",
            "x".repeat(crate::components::response::SYNC_PRETTY_BYTES)
        );
        let tree = crate::components::json_tree::JsonTree::parse(&big).unwrap();

        let mut s = Session::default();
        open(&mut s, "a");
        let generation = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(generation, "a").await);
        assert!(s.arrived(generation, data(&big)));
        assert!(s.response.view().unwrap().parsing);

        // The user moves on while the parse is still running.
        open(&mut s, "b");
        assert!(
            s.tree_arrived(generation, Some(tree)),
            "the parse finds a's cached slot"
        );
        assert!(
            !s.tree_arrived(generation, None),
            "and nothing is left waiting for it"
        );

        open(&mut s, "a");
        let view = s.response.view().unwrap();
        assert!(!view.parsing, "a's response has its tree");
        assert!(view.tree.is_some());
    }

    #[test]
    fn reset_forgets_cached_responses() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response
            .set_state(ResponseState::Ready(data("old project")), 0);
        open(&mut s, "b");

        s.reset();
        open(&mut s, "a");
        assert!(matches!(s.response.state(), ResponseState::Empty));
    }
}
