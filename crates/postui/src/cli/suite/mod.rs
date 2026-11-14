//! `postui suite`: every request of a project, or of one space, sent in
//! sidebar order, one at a time unless `--jobs` asks for more at once, with
//! its status and timing (spec 2026-10-28 §4 "Piece 2 — run"; spec
//! 2026-11-13 §6). A request that can't be read or prepared is that
//! request's result, and the suite goes on. The run tells a [`Watch`] what
//! happens as it happens; the report keeps the rows in sidebar order
//! whatever order they finished in.

mod outline;

use super::mask::Mask;
use super::out::{count, table_under};
use super::read::{displayed, spaces_of, warnings_text};
use super::run::{
    FAILED, FIRST_FAILING_STATUS, RunOpts, millis, prepare_one, refused, session, shown_url,
};
use super::{CliError, Global};
use crate::http::send_raw;
use futures::stream::{FuturesUnordered, StreamExt};
use outline::{Listed, SpaceRequests, outline};
use postui_core::model::Method;
use postui_core::project::Project;
use serde::Serialize;
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

/// `postui suite`. Every string in it has been through the session's mask.
#[derive(Serialize)]
pub struct SuiteReport {
    environment: Option<String>,
    space: Option<String>,
    /// How many requests were sent at once at most (`--jobs`).
    jobs: usize,
    results: Vec<SuiteResult>,
    totals: Totals,
    warnings: Vec<String>,
    /// The absolute path `-o` writes the report to; `null` without `-o`, or
    /// when it could not be written.
    output_file: Option<PathBuf>,
    /// `-o` could not be written (a warning says so): the run failed.
    #[serde(skip)]
    write_failed: bool,
    /// The run was cancelled before every request finished: `results` holds
    /// only those that did.
    #[serde(skip)]
    #[cfg_attr(not(test), expect(dead_code, reason = "wired in by Task 4"))]
    cancelled: bool,
}

/// One request's outcome. `status`, `elapsed_ms` and `size` are `null` when
/// nothing came back; `method` and `url` too when its file is broken.
#[derive(Clone, Serialize)]
pub(super) struct SuiteResult {
    pub(super) request: String,
    pub(super) method: Option<Method>,
    pub(super) url: Option<String>,
    pub(super) status: Option<u16>,
    pub(super) elapsed_ms: Option<u64>,
    pub(super) size: Option<usize>,
    pub(super) error: Option<String>,
}

#[derive(Serialize)]
struct Totals {
    requests: usize,
    passed: usize,
    failed: usize,
    /// The wall-clock time of the run's sends: with several at once, the
    /// sum of the results' own would be longer than the run took.
    elapsed_ms: u64,
}

impl SuiteResult {
    pub(super) fn passed(&self) -> bool {
        self.status.is_some_and(|s| s < FIRST_FAILING_STATUS)
    }
}

/// What a run is about to do, for a [`Watch`]: the environment and space
/// (display names; `space: None` is every space), the `--select`s that
/// differ from the saved selections, how many go at once, the list as the
/// sidebar nests it, and how many requests there are.
#[cfg_attr(not(test), expect(dead_code, reason = "wired in by Task 4"))]
pub(super) struct Plan {
    pub env: Option<String>,
    pub space: Option<String>,
    pub changed: Vec<(String, String)>,
    pub jobs: usize,
    pub lines: Vec<outline::Line>,
    pub total: usize,
}

/// What follows a run as it goes: the live view, or [`Quiet`] for the plain
/// and JSON reports. Requests are numbered by their place in sidebar order.
pub(super) trait Watch {
    /// Before the first request starts.
    fn begin(&mut self, plan: &Plan);
    /// Whether request `index` may start now; asked only while another is
    /// in flight, so one always runs.
    fn may_start(&self, index: usize) -> bool {
        let _ = index;
        true
    }
    /// Request `index` is about to be prepared and sent.
    fn started(&mut self, index: usize) {
        let _ = index;
    }
    /// Request `index` has its result.
    fn finished(&mut self, index: usize, result: &SuiteResult) {
        let _ = (index, result);
    }
    /// Whether [`Watch::tick`] wants calling about ten times a second.
    fn ticks(&self) -> bool {
        false
    }
    fn tick(&mut self) {}
}

/// A [`Watch`] that shows nothing: the plain and JSON reports print once,
/// at the end.
pub struct Quiet;

impl Watch for Quiet {
    fn begin(&mut self, _plan: &Plan) {}
}

/// `slug` as the listing gives it: its method and display name. The slug
/// comes from the listing, so it is always there.
fn listed(project: &Project, slug: String) -> Listed {
    let listing = project.requests().iter().find(|l| l.slug == slug);
    let name = listing.map_or_else(
        || slug.rsplit('/').next().unwrap_or(&slug).to_string(),
        |l| l.display_name().to_string(),
    );
    Listed { method: listing.and_then(|l| l.method), name, slug }
}

/// Runs the requests of `space` (every space when `None`) of the project `g`
/// names, in the order `postui requests` lists them, up to `jobs` at once,
/// telling `watch` as each starts and finishes. When `cancel` completes, no
/// more start, those in flight are dropped, and the report holds only the
/// results that came in. The session's mask comes back beside the report,
/// for what is added to it after the run.
pub async fn suite(
    g: &Global,
    space: Option<&str>,
    opts: &RunOpts,
    jobs: usize,
    watch: &mut dyn Watch,
    cancel: impl Future<Output = ()>,
) -> Result<(SuiteReport, Mask), CliError> {
    let mut s = session(g, opts)?;
    let spaces: Vec<SpaceRequests> = spaces_of(&s.project, space)?
        .into_iter()
        .map(|space| SpaceRequests {
            name: s.project.space_name(&space),
            requests: displayed(&s.project, &space)
                .into_iter()
                .map(|slug| listed(&s.project, slug))
                .collect(),
            slug: space,
        })
        .collect();
    let slugs: Vec<String> =
        spaces.iter().flat_map(|space| space.requests.iter().map(|r| r.slug.clone())).collect();
    // Before anything is sent: a misspelt `--var` would change no request.
    s.check_vars(&slugs, "no request's [variables] has it")?;
    let folder_name = |path: &str| match s.project.folders().iter().find(|f| f.path == path) {
        Some(folder) => folder.display_name().to_string(),
        None => path.rsplit('/').next().unwrap_or(path).to_string(),
    };
    let n = slugs.len();
    let plan = Plan {
        env: s.env.as_deref().map(|env| s.project.env_name(env)),
        space: space.map(|space| s.project.space_name(space)),
        changed: s.changed.clone(),
        jobs,
        lines: outline(&spaces, &folder_name),
        total: n,
    };
    watch.begin(&plan);

    // What `run` would warn of for each request, in suite order: requests
    // are prepared in that order, whatever order they finish in.
    let mut request_warnings = Vec::new();
    let began = Instant::now();
    let mut results: Vec<Option<SuiteResult>> = vec![None; n];
    let mut flight = FuturesUnordered::new();
    let mut next = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tokio::pin!(cancel);
    let mut cancelled = false;
    loop {
        // Requests start in sidebar order. Preparing needs the session, so
        // it stays here, one at a time; only the sends run at once.
        while flight.len() < jobs && next < n && (flight.is_empty() || watch.may_start(next)) {
            let index = next;
            next += 1;
            watch.started(index);
            let slug = &slugs[index];
            let mut result = SuiteResult {
                request: slug.clone(),
                method: None,
                url: None,
                status: None,
                elapsed_ms: None,
                size: None,
                error: None,
            };
            match prepare_one(&mut s, slug) {
                // A broken file is this request's result, naming the file
                // first and absolute, as an error does (R22).
                Err(e) => {
                    let at = e.file.map(|f| format!("{}: ", f.display())).unwrap_or_default();
                    result.error = Some(format!("{at}{}", e.message.trim_end()));
                }
                // The file read; only the variables failed: it keeps its
                // method and URL as written.
                Ok((read, Err(message))) => {
                    let (method, url, error) = refused(&s.mask, &read, &message);
                    (result.method, result.url, result.error) =
                        (Some(method), Some(url), Some(error));
                }
                Ok((_, Ok((request, prepare_warnings)))) => {
                    request_warnings.extend(
                        prepare_warnings.iter().map(|w| s.mask.apply(&format!("{slug}: {w}"))),
                    );
                    result.method = Some(request.method);
                    result.url = Some(shown_url(&request, opts.show_secrets));
                    // The send owns its client (a cheap handle) and its
                    // request, so the next can be prepared while it runs.
                    let client = s.clients.for_request(&request).clone();
                    flight.push(async move {
                        let sent = send_raw(&client, &request).await;
                        (index, result, sent)
                    });
                    continue;
                }
            }
            watch.finished(index, &result);
            results[index] = Some(result);
        }
        if flight.is_empty() && next >= n {
            break;
        }
        tokio::select! {
            Some((index, mut result, sent)) = flight.next() => {
                match sent {
                    Ok(raw) => {
                        result.status = Some(raw.status);
                        result.elapsed_ms = Some(millis(raw.elapsed));
                        result.size = Some(raw.bytes.len());
                    }
                    Err(message) => result.error = Some(s.mask.apply(&message)),
                }
                watch.finished(index, &result);
                results[index] = Some(result);
            }
            _ = tick.tick(), if watch.ticks() => watch.tick(),
            () = &mut cancel => {
                cancelled = true;
                break;
            }
        }
    }
    let wall = millis(began.elapsed());
    let results: Vec<SuiteResult> = results.into_iter().flatten().collect();
    let passed = results.iter().filter(|r| r.passed()).count();
    let totals = Totals {
        requests: results.len(),
        passed,
        failed: results.len() - passed,
        elapsed_ms: wall,
    };
    let report = SuiteReport {
        environment: s.env.clone(),
        space: space.map(str::to_string),
        jobs,
        results,
        totals,
        warnings: s.warnings.iter().map(|w| s.mask.apply(w)).chain(request_warnings).collect(),
        output_file: None,
        write_failed: false,
        cancelled,
    };
    Ok((report, s.mask))
}

impl SuiteReport {
    /// 1 when any request failed to prepare or send or returned 400 or more,
    /// or `-o` could not be written.
    pub fn status(&self) -> ExitCode {
        if self.totals.failed > 0 || self.write_failed {
            ExitCode::from(FAILED)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// Whether the run was cancelled before every request finished.
    #[cfg_attr(not(test), expect(dead_code, reason = "wired in by Task 4"))]
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Names the file `-o` will write the report to, before it is rendered.
    pub fn set_output_file(&mut self, file: PathBuf) {
        self.output_file = Some(file);
    }

    /// Records that `-o` could not be written: the requests were sent, so the
    /// report still prints, with `message` (already masked) as a warning.
    pub fn note_write_failure(&mut self, message: String) {
        self.output_file = None;
        self.warnings.push(message);
        self.write_failed = true;
    }

    /// A heading (naming the jobs when more than one went at once), a row a
    /// request (status, time, method, slug, URL, size) with what failed
    /// under it, the totals, and where `-o` wrote it.
    pub fn text(&self) -> String {
        let mut out = format!(
            "suite  environment {}  space {}",
            self.environment.as_deref().unwrap_or("(none)"),
            self.space.as_deref().unwrap_or("(all)")
        );
        if self.jobs > 1 {
            out.push_str(&format!("  jobs {}", self.jobs));
        }
        out.push('\n');
        let rows: Vec<(Vec<String>, &str)> = self
            .results
            .iter()
            .map(|r| {
                let cells = vec![
                    r.status.map_or_else(|| "error".to_string(), |s| s.to_string()),
                    r.elapsed_ms.map_or_else(String::new, |ms| format!("{ms} ms")),
                    r.method.map_or("", Method::as_str).to_string(),
                    r.request.clone(),
                    r.url.clone().unwrap_or_default(),
                    r.size.map_or_else(String::new, |n| count(n, "byte")),
                ];
                (cells, r.error.as_deref().unwrap_or_default())
            })
            .collect();
        out.push_str(&table_under(&rows));
        let t = &self.totals;
        out.push_str(&format!(
            "\n{} requests, {} passed, {} failed, {} ms\n",
            t.requests, t.passed, t.failed, t.elapsed_ms
        ));
        // Absolute: the file is the user's, named from their working
        // directory, not the project's (R20).
        if let Some(file) = &self.output_file {
            out.push_str(&format!("written to {}\n", file.display()));
        }
        out.push_str(&warnings_text(&self.warnings));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::outline::Line;
    use super::*;
    use std::path::Path;

    /// main: list (broken), users/get "Get user" (refused: `{{nope}}`), in
    /// the folder "Users"; admin ("Admin"): hang, sent to `base`. Selector
    /// `region` (dev's: east saved) and shared `locale` (en saved).
    fn project(root: &Path, base: &str) {
        let put = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put(
            "project.toml",
            "name = \"T\"\nspaces = [\"main\", \"admin\"]\n\n[space.admin]\nname = \"Admin\"\n\n\
             [environment.dev]\nname = \"Dev\"\n",
        );
        put(
            "variables.toml",
            "[base]\n\n[selectors.region]\nfields = [\"host\"]\n\n[selectors.locale]\n\
             fields = [\"lang\"]\nshared = true\n\n[options.locale.en]\nlang = \"en\"\n\n\
             [options.locale.fr]\nlang = \"fr\"\n",
        );
        put(
            "environments/dev.toml",
            &format!(
                "base = \"{base}\"\n\n[options.region.east]\nhost = \"e\"\n\n\
                 [options.region.west]\nhost = \"w\"\n"
            ),
        );
        put(
            ".postui/state.toml",
            "environment = \"dev\"\n\n[selections.dev]\nregion = \"east\"\n\n\
             [shared_selections]\nlocale = \"en\"\n",
        );
        put("requests/main/list.toml", "nope = [");
        put("requests/main/users/.folder.toml", "name = \"Users\"\n");
        put(
            "requests/main/users/get.toml",
            "name = \"Get user\"\nmethod = \"GET\"\nurl = \"{{nope}}/users\"\n",
        );
        put(
            "requests/admin/hang.toml",
            "name = \"Hang\"\nmethod = \"GET\"\nurl = \"{{base}}/h\"\n",
        );
    }

    /// What a [`Plan`] said.
    #[derive(Debug, PartialEq)]
    struct Seen {
        env: Option<String>,
        space: Option<String>,
        changed: Vec<(String, String)>,
        jobs: usize,
        lines: Vec<Line>,
        total: usize,
    }

    /// A [`Watch`] that writes down what it is told, in order.
    #[derive(Default)]
    struct Recorder {
        plan: Option<Seen>,
        events: Vec<String>,
    }

    impl Watch for Recorder {
        fn begin(&mut self, plan: &Plan) {
            self.plan = Some(Seen {
                env: plan.env.clone(),
                space: plan.space.clone(),
                changed: plan.changed.clone(),
                jobs: plan.jobs,
                lines: plan.lines.clone(),
                total: plan.total,
            });
            self.events.push("begin".into());
        }
        fn started(&mut self, index: usize) {
            self.events.push(format!("started {index}"));
        }
        fn finished(&mut self, index: usize, result: &SuiteResult) {
            self.events.push(format!("finished {index} {}", result.request));
        }
    }

    fn opts(selects: &[(&str, &str)]) -> RunOpts {
        RunOpts {
            env: None,
            selects: selects.iter().map(|(s, o)| (s.to_string(), o.to_string())).collect(),
            vars: vec![],
            show_secrets: false,
            timeout: Some(Duration::from_secs(30)),
        }
    }

    fn global(root: &Path) -> Global {
        Global { project: Some(root.to_str().unwrap().into()), json: false }
    }

    /// The watch hears the plan (display names, the `--select`s that differ
    /// from the saved picks, the list as the sidebar nests it) before
    /// anything starts, then each request start and finish in sidebar
    /// order.
    #[tokio::test]
    async fn a_watch_hears_the_plan_then_each_request_in_order() {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), "http://127.0.0.1:9");
        let mut watch = Recorder::default();
        // `locale=en` is the saved pick: not a change.
        let opts = opts(&[("region", "west"), ("locale", "en")]);
        let (report, _) =
            suite(&global(dir.path()), Some("main"), &opts, 2, &mut watch, std::future::pending())
                .await
                .unwrap();
        assert_eq!(
            watch.plan,
            Some(Seen {
                env: Some("Dev".into()),
                space: Some("main".into()),
                changed: vec![("region".into(), "west".into())],
                jobs: 2,
                lines: vec![
                    Line::Space { name: "main".into() },
                    Line::Request { depth: 1, index: 0, method: None, name: "list".into() },
                    Line::Folder { depth: 1, name: "Users".into() },
                    Line::Request {
                        depth: 2,
                        index: 1,
                        method: Some(Method::Get),
                        name: "Get user".into()
                    },
                ],
                total: 2,
            })
        );
        assert_eq!(
            watch.events,
            [
                "begin",
                "started 0",
                "finished 0 main/list",
                "started 1",
                "finished 1 main/users/get"
            ]
        );
        assert!(!report.cancelled());
        assert_eq!(report.results.len(), 2);
    }

    /// Spec 2026-11-13 §5.4: once `cancel` completes nothing more starts,
    /// what is in flight is dropped, and the report holds only what
    /// finished. A shared selector's change is named against its one pick.
    #[tokio::test]
    async fn a_cancelled_run_reports_only_what_finished() {
        // Takes the connection and never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", silent.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), &base);
        let mut watch = Recorder::default();
        let opts = opts(&[("locale", "fr")]);
        let (report, _) =
            suite(&global(dir.path()), None, &opts, 1, &mut watch, std::future::ready(()))
                .await
                .unwrap();
        let plan = watch.plan.unwrap();
        assert_eq!((plan.space, plan.total), (None, 3));
        assert_eq!(plan.changed, [("locale".to_string(), "fr".to_string())]);
        assert_eq!(
            watch.events,
            [
                "begin",
                "started 0",
                "finished 0 main/list",
                "started 1",
                "finished 1 main/users/get",
                "started 2"
            ]
        );
        assert!(report.cancelled());
        let slugs: Vec<&str> = report.results.iter().map(|r| r.request.as_str()).collect();
        assert_eq!(slugs, ["main/list", "main/users/get"]);
        assert_eq!(report.totals.requests, 2);
    }
}
