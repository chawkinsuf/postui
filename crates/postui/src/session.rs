//! The request session: which request's response is on screen, the
//! per-request cache of earlier responses, and the in-flight sends — at
//! most one per request, any number across requests.
//!
//! A response is not app-global — it is *the response to one request*. This
//! module owns that binding: the on-screen [`Response`] always belongs to
//! the open request, every other request's latest response waits in a
//! cache for as long as the session and the request last, and a result
//! that arrives after the user has navigated away lands in its own
//! request's cache slot instead of on screen.

use crate::components::response::{CheckState, Response, ResponseState};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// How many big responses keep their tree and jq state while cached.
/// Switching back to any of the last few is instant; older ones shed that
/// state (`Response::shed_derived`) and re-parse in the background when
/// they return, so a session that has touched many large responses holds
/// a few trees, not one per request.
pub const KEEP_WARM: usize = 3;

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
    /// `open_slug`. Never persisted, and dropped when its request goes
    /// ([`Self::drop_gone`], [`Self::drop_scratch`]).
    cache: HashMap<Option<String>, Response>,
    /// Every send still waiting on its result — at most one entry per
    /// request (`Action::Send` is disabled while its request is in
    /// flight, and `begin_send` defensively supersedes a same-request
    /// entry), any number of entries across requests.
    pub in_flight: Vec<InFlight>,
    /// The cached big responses still holding their derived state, oldest
    /// first; at most `KEEP_WARM` long — pushing past that sheds the
    /// oldest.
    warm: VecDeque<Option<String>>,
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
        let mut outgoing = std::mem::take(&mut self.response);
        // The pane's collapse is a layout preference, not part of any one
        // request's response — it rides along instead of swapping.
        let collapsed = outgoing.collapsed;
        // So does the jq Tab-key mode: it is a config-level UI setting, not
        // part of any one request's response, so it rides along too.
        let jq_tab = outgoing.jq_tab();
        // See `Response::drop_pending_jq`.
        outgoing.drop_pending_jq();
        // The incoming response leaves `warm` before the outgoing one is
        // pushed, so the shed below never picks the one being opened.
        self.warm.retain(|s| s != open);
        // An Empty response carries nothing worth restoring; keeping the
        // cache to requests that actually have a result bounds its size to
        // the requests used this session.
        if !matches!(outgoing.state(), ResponseState::Empty) {
            let big = outgoing.holds_big_derived();
            self.cache.insert(self.open_slug.clone(), outgoing);
            if big {
                // `rename` can have listed it already.
                self.warm.retain(|s| s != &self.open_slug);
                self.warm.push_back(self.open_slug.clone());
                while self.warm.len() > KEEP_WARM {
                    if let Some(old) = self.warm.pop_front()
                        && let Some(cold) = self.cache.get_mut(&old)
                    {
                        cold.shed_derived();
                    }
                }
            }
        }
        self.response = self.cache.remove(open).unwrap_or_default();
        // A guard; see `Response::drop_pending_jq`.
        self.response.drop_pending_jq();
        self.response.collapsed = collapsed;
        self.response.set_jq_tab(jq_tab);
        self.open_slug = open.clone();
        true
    }

    /// A response goes when its request goes (#322; checks tabs spec
    /// 2026-11-16, section 7, the amendment): drops the cached response,
    /// the place in `warm` and the in-flight send of every request
    /// `exists` answers `false` for. A dropped send's result lands
    /// nowhere. Kept, a response would be shown by the next request at
    /// its slug.
    ///
    /// The unsaved scratch has no file: it is gone once another request
    /// is open (#327). A Save as moves what it had to its new name first
    /// ([`Self::rename`] from `None`); a redo of one does not. One scratch
    /// replaced by another
    /// leaves the open slug `None`: that is [`Self::drop_scratch`]'s.
    ///
    /// The open request keeps its send, and its response on screen, while
    /// the editor holds it, whether or not its file is still there: the
    /// editor can send a request whose file was removed outside the app,
    /// and `App::dispatch` calls this after every action, so that send
    /// would be stopped at the next one. The cost: a request put at that
    /// slug while the editor holds the gone one shows that response, at
    /// once when the editor opens it, since the open slug does not
    /// change.
    pub fn drop_gone(&mut self, exists: impl Fn(&str) -> bool) {
        let open = self.open_slug.clone();
        self.drop_where(|slug| slug != &open && slug.as_deref().is_none_or(|s| !exists(s)));
    }

    /// A new unsaved scratch in place of the one before it, under the
    /// same key (#327): drops what the session holds for the old one,
    /// its response on screen or cached, its place in `warm` and its
    /// send. Kept, the new scratch would show that response, or be
    /// sending.
    pub fn drop_scratch(&mut self) {
        if self.open_slug.is_none() {
            self.blank_response();
        }
        self.drop_where(Option::is_none);
    }

    /// Drops the cached response, the place in `warm` and the in-flight
    /// send of every request `gone` picks. A dropped send's task is
    /// aborted rather than left to run: with its entry gone, nothing could
    /// cancel it, and its result lands nowhere. Nothing reports it: the
    /// request it was for is gone.
    fn drop_where(&mut self, gone: impl Fn(&Option<String>) -> bool) {
        self.cache.retain(|slug, _| !gone(slug));
        self.warm.retain(|slug| !gone(slug));
        self.in_flight.retain(|f| {
            let gone = gone(&f.slug);
            if gone {
                f.task.abort();
            }
            !gone
        });
    }

    /// Follows a request whose slug changed on disk (a rename, a move to
    /// another space, or `from` `None`: a Save as giving the unsaved
    /// scratch its first name, #326): the open slug, the cache slot, the
    /// warm list and any in-flight send all move with it, so Send stays
    /// disabled for the renamed request and its late response still lands
    /// on it rather than under a slug some future request would inherit.
    pub fn rename(&mut self, from: Option<&str>, to: &str) {
        let from_key = from.map(str::to_string);
        let to_key = Some(to.to_string());
        if from_key == to_key {
            return;
        }
        if self.open_slug == from_key {
            self.open_slug = to_key.clone();
        }
        if let Some(response) = self.cache.remove(&from_key) {
            self.cache.insert(to_key.clone(), response);
        }
        for slot in self.warm.iter_mut() {
            if *slot == from_key {
                *slot = to_key.clone();
            }
        }
        for inflight in self.in_flight.iter_mut() {
            if inflight.slug == from_key {
                inflight.slug = to_key.clone();
            }
        }
    }

    /// [`Self::rename`] for every request of a renamed space: slugs are
    /// `<space>/<rest>`, so all of them changed at once.
    pub fn rename_space(&mut self, from: &str, to: &str) {
        let from_prefix = format!("{from}/");
        let rekey = |slug: &mut Option<String>| {
            if let Some(rest) = slug.as_deref().and_then(|s| s.strip_prefix(&from_prefix)) {
                *slug = Some(format!("{to}/{rest}"));
            }
        };
        rekey(&mut self.open_slug);
        for slot in self.warm.iter_mut() {
            rekey(slot);
        }
        for inflight in self.in_flight.iter_mut() {
            rekey(&mut inflight.slug);
        }
        let cache = std::mem::take(&mut self.cache);
        for (mut key, response) in cache {
            rekey(&mut key);
            self.cache.insert(key, response);
        }
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
        self.response_for(slug)
            .set_state(ResponseState::InFlight { started: Instant::now() }, generation);
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
        self.response_for(&inflight.slug).set_state(ResponseState::Cancelled, generation);
        true
    }

    /// Delivers a completed response to its owner — on screen when that
    /// request is still open, its cache slot otherwise — with how its
    /// checks stand as it arrives. A result from a superseded generation
    /// is dropped.
    pub fn arrived(
        &mut self,
        generation: u64,
        data: Box<crate::http::ResponseData>,
        checks: CheckState,
    ) -> bool {
        let response = self.deliver(generation, ResponseState::Ready(data));
        response.map(|r| r.set_checks(checks)).is_some()
    }

    /// Like `arrived`, for a send that failed.
    pub fn failed(&mut self, generation: u64, error: String) -> bool {
        self.deliver(generation, ResponseState::Failed(error)).is_some()
    }

    fn deliver(&mut self, generation: u64, state: ResponseState) -> Option<&mut Response> {
        // A result is current exactly while its send is still tracked:
        // cancel and same-request supersession both remove the entry, so
        // a raced or stale result finds nothing and is dropped.
        let i = self.in_flight.iter().position(|f| f.generation == generation)?;
        let slug = self.in_flight.remove(i).slug;
        let response = self.response_for(&slug);
        response.set_state(state, generation);
        Some(response)
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

    /// Gives the response the send `generation` brought its jq checks'
    /// lines (`Action::ChecksJudged`). Found as a finished parse finds it,
    /// on screen or in the cache; when no response is that send's any more
    /// (a new send replaced it) the lines are dropped. Whether the
    /// response on screen took them: only then is there anything to draw.
    pub fn checks_judged(&mut self, generation: u64, lines: Vec<(bool, String)>) -> bool {
        if self.response.is_from(generation) {
            self.response.set_checks(CheckState::Judged(lines));
            return true;
        }
        if let Some(cached) = self.cache.values_mut().find(|r| r.is_from(generation)) {
            cached.set_checks(CheckState::Judged(lines));
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
        self.warm.clear();
        self.blank_response();
        self.open_slug = None;
    }

    /// An empty response on screen in place of the one there.
    fn blank_response(&mut self) {
        // The collapse layout preference survives even a project switch.
        let collapsed = self.response.collapsed;
        // So does the jq Tab-key mode: it is a config-level UI setting
        // (`UiSettings::jq_tab`), not part of any one response, so a
        // project switch must not silently revert it to the default.
        let jq_tab = self.response.jq_tab();
        self.response = Response::default();
        self.response.collapsed = collapsed;
        self.response.set_jq_tab(jq_tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn data(body: &str) -> Box<crate::http::ResponseData> {
        Box::new(crate::http::ResponseData {
            status: 200,
            url: "https://x.test/a".into(),
            headers: vec![],
            sent_headers: vec![],
            body: body.to_string().into(),
            ttfb: Duration::from_millis(1),
            elapsed: Duration::from_millis(1),
            size: body.len(),
            content_type: None,
        })
    }

    /// A response to a request without checks arrives.
    fn arrives(s: &mut Session, generation: u64, body: &str) -> bool {
        s.arrived(generation, data(body), CheckState::None)
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

    /// [`in_flight`] with a task that runs until it is aborted.
    async fn running(generation: u64, slug: &str) -> InFlight {
        InFlight { task: tokio::spawn(std::future::pending()), ..in_flight(generation, slug).await }
    }

    /// A JSON body over `SYNC_PRETTY_BYTES`, told apart by `n`.
    fn big(n: usize) -> String {
        let pad = "x".repeat(crate::components::response::SYNC_PRETTY_BYTES);
        format!(r#"{{"pad": "{pad}", "n": {n}}}"#)
    }

    /// Opens `big{n}` and gives it a big response with its tree, on screen.
    fn open_parsed_big(s: &mut Session, n: usize) {
        use crate::components::json_tree::JsonTree;
        open(s, &format!("big{n}"));
        let body = big(n);
        s.response.set_state(ResponseState::Ready(data(&body)), n as u64);
        assert!(s.response.attach_tree(n as u64, JsonTree::parse(&body)));
    }

    #[test]
    fn big_responses_beyond_the_last_few_shed_their_trees_and_reparse_on_return() {
        use crate::components::json_tree::JsonTree;
        let mut s = Session::default();
        // A small response and KEEP_WARM + 1 big ones, each parsed.
        open(&mut s, "small");
        s.response.set_state(ResponseState::Ready(data("{\"n\": 0}")), 0);
        for i in 1..=KEEP_WARM + 1 {
            open_parsed_big(&mut s, i);
        }
        open(&mut s, "elsewhere");
        let has_tree = |s: &Session, slug: &str| {
            s.cache[&Some(slug.to_string())].view().is_some_and(|v| v.tree.is_some())
        };
        assert!(has_tree(&s, "small"), "a small response is never shed");
        assert!(!has_tree(&s, "big1"), "the oldest big one past KEEP_WARM sheds its tree");
        for i in 2..=KEEP_WARM + 1 {
            assert!(has_tree(&s, &format!("big{i}")), "big{i} stays warm");
        }
        assert_eq!(
            body_of(&s.cache[&Some("big1".into())]).map(str::len),
            Some(big(1).len()),
            "the body itself is kept"
        );
        // A warm one returns as it was: nothing to re-parse.
        open(&mut s, "big3");
        assert!(s.response.view().unwrap().tree.is_some());
        assert_eq!(s.response.take_reparse(), None);
        // The shed one returns parsing, and hands its body back once.
        open(&mut s, "big1");
        let view = s.response.view().unwrap();
        assert!(view.tree.is_none() && view.parsing, "shed: parsing again");
        let (generation, body) = s.response.take_reparse().expect("needs a parse");
        assert_eq!((generation, body.len()), (1, big(1).len()));
        assert_eq!(s.response.take_reparse(), None, "handed out once");
        assert!(s.response.attach_tree(generation, JsonTree::parse(&body)));
        assert!(s.response.view().unwrap().tree.is_some(), "the tree is back");
    }

    /// #321: the response being opened is never the one shed on its way in.
    #[test]
    fn going_straight_back_to_the_oldest_warm_response_keeps_its_tree() {
        let mut s = Session::default();
        for i in 1..=KEEP_WARM + 1 {
            open_parsed_big(&mut s, i);
        }
        let slugs = |names: &[&str]| -> Vec<Option<String>> {
            names.iter().map(|n| Some(n.to_string())).collect()
        };
        // `warm` is `names`. Every big response here got its tree while on
        // screen, so the cached ones still holding a tree are `names` too.
        let warm_ok = |s: &Session, names: &[&str]| {
            let mut holding: Vec<_> = s
                .cache
                .iter()
                .filter(|(_, r)| r.holds_big_derived())
                .map(|(k, _)| k.clone())
                .collect();
            holding.sort();
            let mut want = slugs(names);
            want.sort();
            assert_eq!(s.warm.iter().cloned().collect::<Vec<_>>(), slugs(names));
            assert_eq!(holding, want, "the cached ones holding a tree");
        };
        warm_ok(&s, &["big1", "big2", "big3"]);
        // Straight from the newest back to the oldest of the last few.
        open(&mut s, "big1");
        let view = s.response.view().unwrap();
        assert!(view.tree.is_some() && !view.parsing, "big1 was warm: no re-parse");
        assert_eq!(s.response.take_reparse(), None);
        warm_ok(&s, &["big2", "big3", "big4"]);
        open(&mut s, "elsewhere");
        warm_ok(&s, &["big3", "big4", "big1"]);
        // From an Empty response, and from a small one, the response opened
        // leaves `warm` too.
        open(&mut s, "big3");
        warm_ok(&s, &["big4", "big1"]);
        open(&mut s, "small");
        s.response.set_state(ResponseState::Ready(data("{\"n\": 0}")), 9);
        open(&mut s, "big4");
        warm_ok(&s, &["big1", "big3"]);
    }

    /// #322: a request that is gone takes its cached response, its place
    /// in `warm` and its send, whose task is aborted and whose result
    /// then lands nowhere. The open request and the requests still there
    /// keep theirs. The unsaved scratch, left for another request, is
    /// gone (#327).
    #[tokio::test]
    async fn a_request_that_is_gone_takes_its_response_its_warm_place_and_its_send() {
        let mut s = Session::default();
        s.response.set_state(ResponseState::Ready(data("scratch")), 0);
        open_parsed_big(&mut s, 1);
        open_parsed_big(&mut s, 2);
        open(&mut s, "sent");
        let gone = s.begin_send(&Some("sent".into()));
        s.in_flight.push(running(gone, "sent").await);
        open(&mut s, "kept");
        let kept = s.begin_send(&Some("kept".into()));
        s.in_flight.push(running(kept, "kept").await);
        open(&mut s, "open");
        let open_send = s.begin_send(&Some("open".into()));
        s.in_flight.push(running(open_send, "open").await);
        let task = |s: &Session, slug: &str| {
            s.in_flight_for(&Some(slug.into())).expect("in flight").task.abort_handle()
        };
        let (sent_task, kept_task, open_task) =
            (task(&s, "sent"), task(&s, "kept"), task(&s, "open"));

        // `big1`, `sent` and the open request have no file any more.
        s.drop_gone(|slug| ["big2", "kept"].contains(&slug));
        tokio::task::yield_now().await;
        assert!(sent_task.is_finished(), "its task is aborted");
        assert!(!kept_task.is_finished() && !open_task.is_finished(), "the others run on");
        assert_eq!(s.warm.iter().flatten().collect::<Vec<_>>(), ["big2"]);
        assert!(!s.is_in_flight(&Some("sent".into())));
        assert!(!arrives(&mut s, gone, "late"), "its result is dropped");
        assert!(arrives(&mut s, open_send, "open's"), "the open request's send still lands");
        assert_eq!(body_of(&s.response), Some("open's"));
        for slug in ["big1", "sent"] {
            open(&mut s, slug);
            assert!(matches!(s.response.state(), ResponseState::Empty), "{slug}");
        }
        assert!(arrives(&mut s, kept, "theirs"), "a request still there keeps its send");
        open(&mut s, "kept");
        assert_eq!(body_of(&s.response), Some("theirs"));
        open(&mut s, "big2");
        assert!(body_of(&s.response) == Some(big(2).as_str()), "and its response");
        s.sync_open(&None);
        assert!(matches!(s.response.state(), ResponseState::Empty), "the scratch is gone");
    }

    /// A renamed request's response, its place in `warm` and its send go
    /// with it, on screen or cached.
    #[tokio::test]
    async fn a_renamed_request_keeps_its_response_its_warm_place_and_its_send() {
        let mut s = Session::default();
        open_parsed_big(&mut s, 1);
        open(&mut s, "sent");
        let generation = s.begin_send(&Some("sent".into()));
        s.in_flight.push(in_flight(generation, "sent").await);
        s.rename(Some("sent"), "moved");
        s.rename(Some("big1"), "big9");
        assert_eq!(s.warm.iter().flatten().collect::<Vec<_>>(), ["big9"]);
        assert!(s.is_in_flight(&Some("moved".into())));
        assert!(arrives(&mut s, generation, "late"));
        assert_eq!(body_of(&s.response), Some("late"), "on screen, under its new slug");
        open(&mut s, "big9");
        assert!(body_of(&s.response) == Some(big(1).as_str()));
        assert!(s.response.view().unwrap().tree.is_some(), "still warm");
        open(&mut s, "moved");
        assert_eq!(body_of(&s.response), Some("late"));
    }

    /// #326: the unsaved scratch saved as a request is the same request:
    /// its response and its send go with it, and the next scratch starts
    /// with neither.
    #[tokio::test]
    async fn a_scratch_saved_as_a_request_takes_its_response_and_its_send() {
        let mut s = Session::default();
        let generation = s.begin_send(&None);
        s.in_flight.push(InFlight { slug: None, ..in_flight(generation, "unused").await });
        s.rename(None, "kept");
        assert!(!s.sync_open(&Some("kept".into())), "already open: nothing to swap");
        assert!(s.is_in_flight(&Some("kept".into())) && !s.is_in_flight(&None));
        assert!(arrives(&mut s, generation, "late"));
        assert_eq!(body_of(&s.response), Some("late"), "on screen, under its new slug");
        s.sync_open(&None);
        assert!(matches!(s.response.state(), ResponseState::Empty), "a fresh scratch");
        open(&mut s, "kept");
        assert_eq!(body_of(&s.response), Some("late"));
    }

    /// #327: an unsaved scratch left for another request, or replaced by
    /// a new one, is gone: what the session holds for it goes, its
    /// response on screen or cached, its place in `warm` and its send,
    /// whose task is aborted and whose result then lands nowhere. Other
    /// requests keep theirs.
    #[tokio::test]
    async fn a_scratch_left_or_replaced_takes_its_response_and_send() {
        let scratch_send = |s: &mut Session| {
            let generation = s.begin_send(&None);
            let task = tokio::spawn(std::future::pending());
            let handle = task.abort_handle();
            s.in_flight.push(InFlight { started: Instant::now(), generation, slug: None, task });
            (generation, handle)
        };
        // Replaced by a new scratch: on screen, the open slug stays `None`.
        let mut s = Session::default();
        s.response.collapsed = true;
        let (generation, task) = scratch_send(&mut s);
        s.drop_gone(|_| false);
        assert!(s.is_in_flight(&None), "open, it is kept");
        s.drop_scratch();
        tokio::task::yield_now().await;
        assert!(task.is_finished(), "its task is aborted");
        assert!(!s.is_in_flight(&None));
        assert!(matches!(s.response.state(), ResponseState::Empty));
        assert!(s.response.collapsed, "the pane's layout stays");
        assert!(!arrives(&mut s, generation, "late"), "its result is dropped");

        // Left for another request, with its send still out.
        let mut s = Session::default();
        let (generation, task) = scratch_send(&mut s);
        open(&mut s, "other");
        s.response.set_state(ResponseState::Ready(data("other's")), 0);
        s.drop_gone(|_| true);
        tokio::task::yield_now().await;
        assert!(task.is_finished(), "its task is aborted");
        assert!(!arrives(&mut s, generation, "late"), "its result is dropped");
        assert_eq!(body_of(&s.response), Some("other's"), "the open request keeps its own");
        s.sync_open(&None);
        assert!(matches!(s.response.state(), ResponseState::Empty), "a fresh scratch");

        // Left, with a big response: it leaves its place in `warm` too.
        let mut s = Session::default();
        let body = big(0);
        s.response.set_state(ResponseState::Ready(data(&body)), 0);
        assert!(s.response.attach_tree(0, crate::components::json_tree::JsonTree::parse(&body)));
        open(&mut s, "other");
        assert_eq!(s.warm.len(), 1, "the scratch's response is warm");
        s.drop_gone(|_| true);
        assert!(s.warm.is_empty(), "{:?}", s.warm);
        s.sync_open(&None);
        assert!(matches!(s.response.state(), ResponseState::Empty), "a fresh scratch");
    }

    #[test]
    fn switching_open_request_swaps_to_cached_or_empty_response() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response.set_state(ResponseState::Ready(data("from a")), 0);

        open(&mut s, "b");
        assert!(
            matches!(s.response.state(), ResponseState::Empty),
            "a request never sent shows an empty response, not a's leftovers"
        );
        s.response.set_state(ResponseState::Ready(data("from b")), 0);

        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("from a"));
        open(&mut s, "b");
        assert_eq!(body_of(&s.response), Some("from b"));
    }

    #[test]
    fn collapsed_is_a_layout_preference_that_sticks_across_request_switches() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response.set_state(ResponseState::Ready(data("from a")), 0);
        s.response.collapsed = true;

        open(&mut s, "b");
        assert!(s.response.collapsed, "collapse follows to the next request");

        s.response.collapsed = false;
        open(&mut s, "a");
        assert!(
            !s.response.collapsed,
            "re-expanding sticks too — a's cached response must not bring the old flag back"
        );
    }

    #[test]
    fn jq_tab_is_a_config_setting_that_sticks_across_request_switches() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response.set_state(ResponseState::Ready(data("from a")), 0);
        s.response.set_jq_tab(crate::config::JqTab::Cycle);

        open(&mut s, "b");
        assert_eq!(
            s.response.jq_tab(),
            crate::config::JqTab::Cycle,
            "jq_tab follows to a request opened for the first time this session"
        );

        open(&mut s, "a");
        assert_eq!(
            s.response.jq_tab(),
            crate::config::JqTab::Cycle,
            "…and back to a's cached response too"
        );
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
        assert!(arrives(&mut s, generation, "late result"));
        assert!(
            matches!(s.response.state(), ResponseState::Empty),
            "b's screen must not show a's late result"
        );

        open(&mut s, "a");
        assert_eq!(body_of(&s.response), Some("late result"));
    }

    /// Checks tabs spec 2026-11-16, section 7 (A5): a send's check lines
    /// land on the response that send brought, on screen or cached, a shed
    /// one included, and nowhere once it is replaced or forgotten.
    #[tokio::test]
    async fn check_lines_find_the_response_their_send_brought() {
        use crate::components::json_tree::JsonTree;
        use crate::components::response::{SYNC_PRETTY_BYTES, ViewMode};
        let lines = |text: &str| vec![(true, text.to_string())];
        let checks_tab = |r: &mut Response| {
            r.set_view_mode(ViewMode::Checks);
            r.view().filter(|v| v.mode == ViewMode::Checks).map(|v| v.view_text())
        };
        let big = format!(r#"{{"pad": "{}"}}"#, "x".repeat(SYNC_PRETTY_BYTES));
        let mut s = Session::default();
        let mut sent = Vec::new();
        // Big responses waiting on their checks, enough of them that the
        // first is shed in the cache.
        for i in 0..=KEEP_WARM {
            let slug = format!("big{i}");
            open(&mut s, &slug);
            let generation = s.begin_send(&Some(slug.clone()));
            s.in_flight.push(in_flight(generation, &slug).await);
            assert!(s.arrived(generation, data(&big), CheckState::Judging));
            assert!(s.tree_arrived(generation, JsonTree::parse(&big)));
            sent.push(generation);
        }
        open(&mut s, "elsewhere");
        assert!(s.cache[&Some("big0".into())].view().unwrap().tree.is_none(), "shed");
        assert!(!s.checks_judged(sent[0], lines(".a")), "off screen: nothing to draw");
        assert_eq!(checks_tab(&mut s.response), None, "not the request on screen");
        open(&mut s, "big1");
        let waiting = Some("checking\u{2026}");
        assert_eq!(checks_tab(&mut s.response).as_deref(), waiting, "nor another send's");
        assert!(s.checks_judged(sent[1], lines(".b")), "on screen");
        assert_eq!(checks_tab(&mut s.response).as_deref(), Some("✓ .b"));
        open(&mut s, "big0");
        assert_eq!(checks_tab(&mut s.response).as_deref(), Some("✓ .a"));
        // A new send of the request: the earlier send's lines find nothing.
        let again = s.begin_send(&Some("big0".into()));
        s.in_flight.push(in_flight(again, "big0").await);
        assert!(!s.checks_judged(sent[0], lines("stale")));
        assert!(s.arrived(again, data("{}"), CheckState::Judging));
        assert!(!s.checks_judged(sent[0], lines("stale")));
        assert_eq!(checks_tab(&mut s.response).as_deref(), waiting);
        // A project switch forgets every response.
        s.reset();
        assert!(!s.checks_judged(again, lines("late")));
        assert!(!s.checks_judged(sent[1], lines("late")));
    }

    #[tokio::test]
    async fn result_from_a_superseded_generation_is_dropped() {
        let mut s = Session::default();
        open(&mut s, "a");
        let stale = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(stale, "a").await);
        let current = s.begin_send(&Some("a".into()));
        s.in_flight.push(in_flight(current, "a").await);

        assert!(!arrives(&mut s, stale, "stale"));
        assert!(
            matches!(s.response.state(), ResponseState::InFlight { .. }),
            "the newer send is still pending"
        );
        assert!(arrives(&mut s, current, "fresh"));
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
        assert!(s.is_in_flight(&Some("a".into())), "b's send leaves a's running");
        assert!(s.is_in_flight(&Some("b".into())));

        assert!(arrives(&mut s, first, "from a"), "a's result still lands");
        assert!(arrives(&mut s, second, "from b"));
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

        assert!(arrives(&mut s, first, "from a"));
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
            !arrives(&mut s, generation, "raced past the abort"),
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
        assert!(!arrives(&mut s, first, "late"));
        assert!(!arrives(&mut s, second, "late"));
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
        assert!(arrives(&mut s, generation, &big));
        assert!(s.response.view().unwrap().parsing);

        // The user moves on while the parse is still running.
        open(&mut s, "b");
        assert!(s.tree_arrived(generation, Some(tree)), "the parse finds a's cached slot");
        assert!(!s.tree_arrived(generation, None), "and nothing is left waiting for it");

        open(&mut s, "a");
        let view = s.response.view().unwrap();
        assert!(!view.parsing, "a's response has its tree");
        assert!(view.tree.is_some());
    }

    #[test]
    fn reset_forgets_cached_responses() {
        let mut s = Session::default();
        open(&mut s, "a");
        s.response.set_state(ResponseState::Ready(data("old project")), 0);
        open(&mut s, "b");

        s.reset();
        open(&mut s, "a");
        assert!(matches!(s.response.state(), ResponseState::Empty));
    }

    #[test]
    fn jq_tab_is_a_config_setting_that_survives_a_project_switch() {
        let mut s = Session::default();
        s.response.set_jq_tab(crate::config::JqTab::Cycle);

        s.reset();

        assert_eq!(
            s.response.jq_tab(),
            crate::config::JqTab::Cycle,
            "jq_tab is a UI setting, not part of the response being reset"
        );
    }
}
