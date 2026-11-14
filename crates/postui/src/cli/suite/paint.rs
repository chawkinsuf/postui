//! The live view's rows, header and totals as styled segments (spec
//! 2026-11-13 §5.1, §5.3). A row is a list of [`Seg`]s; [`fit`] clips it to
//! the terminal's width by display width, and [`paint`] turns it into a
//! string, with the 16 ANSI colors or without any escape code. Nothing here
//! touches a terminal, so the rows are tested as text.

use super::outline::Line;
use super::{Plan, SuiteResult};
use crate::ui::{cells, graphemes};
use postui_core::model::Method;
use std::time::{Duration, Instant};

/// How a segment is styled. Each maps to one SGR code ([`sgr`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Tone {
    Dim,
    Bold,
    Space,
    Folder,
    Ok,
    Redirect,
    Bad,
    NotSent,
    Spin,
    Method(Method),
    BadgeOk,
    BadgeFail,
    BadgeCancel,
}

/// A run of text in one tone. `shrink` is 0 for a segment [`fit`] never
/// shortens; otherwise the order it is shortened in, 1 first.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Seg {
    pub text: String,
    pub tone: Option<Tone>,
    pub shrink: u8,
}

/// Where a request is in the run, as its row shows it.
pub(super) enum RowState {
    Waiting,
    /// Sent and not back; `url` is the masked URL, once it is known.
    Running {
        since: Instant,
        url: Option<String>,
    },
    Done(SuiteResult),
    Skipped,
}

/// The in-flight icon's frames, about ten a second.
pub(super) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The widest a request name's column gets; a longer name is clipped with `…`.
const NAME_CAP: usize = 32;
/// Method column width: `OPTIONS` is the longest and is seven, but the
/// sidebar's rows pad to six (spec §5.1).
const METHOD_COL: usize = 6;
/// Width of the time and size columns: every [`time`] under 1000 s and
/// every [`size`] under 1000 MB fits it (spec §5.1's fixed-width forms).
const NUM_COL: usize = 6;
/// Width of the status column.
const STATUS_COL: usize = 3;

/// The SGR parameter for `tone`.
pub(super) fn sgr(tone: Tone) -> &'static str {
    match tone {
        Tone::Dim => "90",
        Tone::Bold => "1",
        Tone::Space => "1;34",
        Tone::Folder => "34",
        Tone::Ok => "32",
        Tone::Redirect => "33",
        Tone::Bad => "31",
        Tone::NotSent => "35",
        Tone::Spin => "34",
        Tone::Method(Method::Get) => "32",
        Tone::Method(Method::Post) => "36",
        Tone::Method(Method::Put | Method::Patch) => "33",
        Tone::Method(Method::Delete) => "31",
        Tone::Method(Method::Head | Method::Options) => "90",
        Tone::BadgeOk => "1;30;42",
        Tone::BadgeFail => "1;97;41",
        Tone::BadgeCancel => "1;30;43",
    }
}

/// `segs` as one string: each toned segment wrapped in its SGR code when
/// `color`, plain text otherwise.
pub(super) fn paint(segs: &[Seg], color: bool) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg.tone {
            Some(tone) if color && !seg.text.is_empty() => {
                out.push_str("\x1b[");
                out.push_str(sgr(tone));
                out.push('m');
                out.push_str(&seg.text);
                out.push_str("\x1b[0m");
            }
            _ => out.push_str(&seg.text),
        }
    }
    out
}

/// The display width of `segs`.
pub(super) fn width(segs: &[Seg]) -> usize {
    segs.iter().map(|s| cells(&s.text)).sum()
}

/// `text` cut to at most `cols` columns: whole graphemes, the last column an
/// `…` when something was cut. Empty when not even one grapheme and the `…`
/// fit.
fn clip(text: &str, cols: usize) -> String {
    if cells(text) <= cols {
        return text.to_string();
    }
    if cols < 2 {
        return String::new();
    }
    let mut kept = String::new();
    let mut used = 0;
    for (_, g, w) in graphemes(text) {
        if used + w + 1 > cols {
            break;
        }
        kept.push_str(g);
        used += w;
    }
    let kept = kept.trim_end();
    if kept.is_empty() { String::new() } else { format!("{kept}…") }
}

/// Shortens `segs` until they are at most `cols` wide. The segment with the
/// lowest non-zero `shrink` goes first, losing graphemes from its end and
/// ending in `…`; one that can't keep a grapheme and the `…` is emptied and
/// the next goes. If that is not enough, the end of the row is cut.
pub(super) fn fit(segs: &mut Vec<Seg>, cols: usize) {
    while width(segs) > cols {
        let next = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.shrink > 0 && !s.text.is_empty())
            .min_by_key(|(_, s)| s.shrink)
            .map(|(at, _)| at);
        let Some(at) = next else { break };
        let excess = width(segs) - cols;
        let have = cells(&segs[at].text);
        segs[at].text = clip(&segs[at].text, have.saturating_sub(excess));
    }
    // Still too wide: cut from the end, whole graphemes.
    while width(segs) > cols {
        let Some(last) = segs.iter_mut().rev().find(|s| !s.text.is_empty()) else { break };
        let cut = graphemes(&last.text).next_back().map_or(0, |(_, g, _)| g.len());
        let keep = last.text.len() - cut;
        last.text.truncate(keep);
    }
    segs.retain(|s| !s.text.is_empty());
}

/// The widest request name in `lines`, at most 32 columns.
pub(super) fn name_column(lines: &[Line]) -> usize {
    lines
        .iter()
        .filter_map(|l| match l {
            Line::Request { name, .. } => Some(cells(&printable(name))),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(NAME_CAP)
}

fn seg(text: impl Into<String>, tone: Option<Tone>, shrink: u8) -> Seg {
    Seg { text: text.into(), tone, shrink }
}

fn plain(text: impl Into<String>) -> Seg {
    seg(text, None, 0)
}

fn toned(text: impl Into<String>, tone: Tone) -> Seg {
    seg(text, Some(tone), 0)
}

/// `text` padded with spaces on the right to `cols` display columns.
fn pad_right(text: &str, cols: usize) -> String {
    format!("{text}{}", " ".repeat(cols.saturating_sub(cells(text))))
}

/// `text` padded with spaces on the left to `cols` display columns.
fn pad_left(text: &str, cols: usize) -> String {
    format!("{}{text}", " ".repeat(cols.saturating_sub(cells(text))))
}

/// `value` in `unit` in at most three digits: one decimal under 10 (`2.3`),
/// whole from 10 (`12`, `123`). `None` from 1000 on, where the next unit
/// takes over.
fn short(value: f64, unit: &str) -> Option<String> {
    let tenths = (value * 10.0).round() as u64;
    if tenths < 100 {
        return Some(format!("{}.{} {unit}", tenths / 10, tenths % 10));
    }
    let whole = value.round() as u64;
    (whole < 1000).then(|| format!("{whole} {unit}"))
}

/// `ms` as the rows show a time, at most 6 columns under 1000 s: `120 ms`,
/// `2.3 s`, `12 s`, `123 s` (R18).
pub(super) fn time(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let secs = ms as f64 / 1000.0;
    short(secs, "s").unwrap_or_else(|| format!("{} s", secs.round() as u64))
}

/// `bytes` as the rows show a size, at most 6 columns under 1000 MB:
/// `512 B`, `1.2 KB`, `123 KB`, `14 MB`. Each unit gives way to the next at
/// 1000 of it, so `1000 KB` is `1.0 MB` (R18).
pub(super) fn size(bytes: usize) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    for unit in ["KB", "MB"] {
        match short(value, unit) {
            Some(text) => return text,
            None => value /= 1024.0,
        }
    }
    short(value, "GB").unwrap_or_else(|| format!("{} GB", value.round() as u64))
}

/// `text` with each control character (a newline, a tab, ESC) shown as `?`.
/// Display names come from a project's files: shown raw, a newline or tab
/// would break the one-row-one-line count the redraw relies on, and an
/// escape sequence would reach the terminal.
pub(super) fn printable(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn status_tone(status: u16) -> Tone {
    match status {
        ..300 => Tone::Ok,
        300..400 => Tone::Redirect,
        _ => Tone::Bad,
    }
}

/// One line of the list as segments. `state` is `None` for a space or folder
/// line; `now` and `frame` drive a running row's time and spinner.
pub(super) fn line_row(
    line: &Line,
    state: Option<&RowState>,
    name_col: usize,
    now: Instant,
    frame: usize,
) -> Vec<Seg> {
    let (depth, method, name) = match line {
        Line::Space { name } => return vec![toned(printable(name), Tone::Space)],
        Line::Folder { depth, name } => {
            let name = printable(name);
            return vec![plain("  ".repeat(*depth)), toned(format!("{name}/"), Tone::Folder)];
        }
        Line::Request { depth, method, name, .. } => (*depth, *method, printable(name)),
    };
    let state = state.unwrap_or(&RowState::Waiting);
    let dim = matches!(state, RowState::Waiting | RowState::Skipped);
    // A waiting or skipped row is dim from end to end.
    let tone = |t: Tone| if dim { Some(Tone::Dim) } else { Some(t) };
    let icon = match state {
        RowState::Waiting => ("·", Tone::Dim),
        RowState::Skipped => ("–", Tone::Dim),
        RowState::Running { .. } => (SPINNER[frame % SPINNER.len()], Tone::Spin),
        RowState::Done(r) if r.status.is_none() => ("!", Tone::NotSent),
        RowState::Done(r) if r.passed() => ("✓", Tone::Ok),
        RowState::Done(_) => ("✗", Tone::Bad),
    };
    let mut row = vec![
        seg("  ".repeat(depth), dim.then_some(Tone::Dim), 0),
        seg(icon.0, Some(icon.1), 0),
        seg(" ", dim.then_some(Tone::Dim), 0),
    ];
    let method_text = pad_right(method.map_or("", Method::as_str), METHOD_COL);
    row.push(seg(
        method_text,
        method.and_then(|m| tone(Tone::Method(m))).or(dim.then_some(Tone::Dim)),
        0,
    ));
    row.push(seg(" ", dim.then_some(Tone::Dim), 0));
    row.push(seg(pad_right(&clip(&name, name_col), name_col), dim.then_some(Tone::Dim), 2));
    match state {
        RowState::Waiting => {}
        RowState::Skipped => {
            row.push(toned("  ", Tone::Dim));
            row.push(toned("skipped", Tone::Dim));
        }
        RowState::Running { since, url } => {
            let ms = now.saturating_duration_since(*since).as_millis() as u64;
            row.push(plain("  "));
            row.push(plain(" ".repeat(STATUS_COL)));
            row.push(plain("  "));
            row.push(toned(pad_left(&time(ms), NUM_COL), Tone::Spin));
            if let Some(url) = url {
                // The gap belongs to the URL, so a clipped-away URL leaves
                // no trailing spaces. 10 columns line it up with a final
                // row's (`"  "`, size, `"  "`).
                row.push(seg(format!("{}{url}", " ".repeat(10)), Some(Tone::Dim), 1));
            }
        }
        RowState::Done(r) => {
            row.push(plain("  "));
            let Some(status) = r.status else {
                let first = r.error.as_deref().and_then(|e| e.lines().next()).unwrap_or("");
                row.push(seg(first, Some(Tone::NotSent), 1));
                return row;
            };
            row.push(toned(format!("{status:>STATUS_COL$}"), status_tone(status)));
            row.push(plain("  "));
            let ms = r.elapsed_ms.map(time).unwrap_or_default();
            row.push(toned(pad_left(&ms, NUM_COL), Tone::Dim));
            row.push(plain("  "));
            let bytes = r.size.map(size).unwrap_or_default();
            row.push(toned(pad_left(&bytes, NUM_COL), Tone::Dim));
            if let Some(url) = &r.url {
                row.push(seg(format!("  {url}"), Some(Tone::Dim), 1));
            }
        }
    }
    row
}

/// The lines of a final `!` row's message after the first, each indented 4
/// more than the row (`depth` levels in).
pub(super) fn error_under(state: &RowState, depth: usize) -> Vec<String> {
    let RowState::Done(r) = state else { return Vec::new() };
    if r.status.is_some() {
        return Vec::new();
    }
    let indent = " ".repeat(depth * 2 + 4);
    r.error
        .as_deref()
        .map(|e| e.lines().skip(1).map(|l| format!("{indent}{l}")).collect())
        .unwrap_or_default()
}

/// The line above the list: what the run is, and how many go at once.
pub(super) fn header(plan: &Plan) -> Vec<Seg> {
    let mut row = vec![
        toned("postui suite", Tone::Bold),
        toned("  env ", Tone::Dim),
        plain(printable(plan.env.as_deref().unwrap_or("(none)"))),
        toned("  space ", Tone::Dim),
        plain(printable(plan.space.as_deref().unwrap_or("all"))),
    ];
    for (selector, option) in &plan.changed {
        row.push(toned(format!("  {} ", printable(selector)), Tone::Dim));
        row.push(plain(printable(option)));
    }
    let at_once = if plan.jobs > 1 {
        format!("{} at a time", plan.jobs)
    } else {
        "one at a time".to_string()
    };
    row.push(toned(format!(" · {} requests · {at_once}", plan.total), Tone::Dim));
    row
}

/// The line under the list while the run goes.
pub(super) fn progress(
    done: usize,
    total: usize,
    passed: usize,
    failed: usize,
    elapsed: Duration,
) -> Vec<Seg> {
    vec![
        toned(format!(" {done} of {total} · "), Tone::Dim),
        toned(format!("{passed} passed"), Tone::Ok),
        toned(" · ", Tone::Dim),
        toned(format!("{failed} failed"), Tone::Bad),
        toned(format!(" · {}", time(elapsed.as_millis() as u64)), Tone::Dim),
    ]
}

/// How the run ended.
pub(super) enum Badge {
    Ok,
    Fail,
    Cancelled,
}

/// The line that ends the run: a badge and the totals.
pub(super) fn totals(
    badge: Badge,
    total: usize,
    passed: usize,
    failed: usize,
    skipped: usize,
    elapsed: Duration,
) -> Vec<Seg> {
    let (label, tone) = match badge {
        Badge::Ok => (" OK ", Tone::BadgeOk),
        Badge::Fail => (" FAIL ", Tone::BadgeFail),
        Badge::Cancelled => (" CANCELLED ", Tone::BadgeCancel),
    };
    let mut row = vec![
        plain(" "),
        toned(label, tone),
        plain(format!("  {total} requests · ")),
        toned(format!("{passed} passed"), Tone::Ok),
        toned(" · ", Tone::Dim),
        toned(format!("{failed} failed"), Tone::Bad),
    ];
    if skipped > 0 {
        row.push(toned(format!(" · {skipped} skipped"), Tone::Dim));
    }
    row.push(toned(format!(" · {}", time(elapsed.as_millis() as u64)), Tone::Dim));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: Option<u16>, error: Option<&str>) -> SuiteResult {
        SuiteResult {
            request: "main/get-user".into(),
            method: Some(Method::Get),
            url: Some("https://dev/users/42".into()),
            status,
            elapsed_ms: status.map(|_| 120),
            size: status.map(|_| 512),
            error: error.map(Into::into),
        }
    }

    fn request(depth: usize, method: Option<Method>, name: &str) -> Line {
        Line::Request { depth, index: 0, method, name: name.into() }
    }

    fn row(line: &Line, state: &RowState, name_col: usize) -> Vec<Seg> {
        line_row(line, Some(state), name_col, Instant::now(), 0)
    }

    fn long_row() -> Vec<Seg> {
        let mut r = result(Some(200), None);
        r.url = Some("https://example.com/a/very/long/path/that/goes/on/and/on".into());
        row(
            &request(2, Some(Method::Post), "A rather long request name here"),
            &RowState::Done(r),
            31,
        )
    }

    fn plan(changed: Vec<(String, String)>, jobs: usize) -> Plan {
        Plan { env: Some("Dev".into()), space: None, changed, jobs, lines: Vec::new(), total: 9 }
    }

    /// Spec 2026-11-13 §5.1's fixed-width forms (R18): a decimal only under
    /// 10, and the next unit from 1000 of one, so every realistic time and
    /// size fits the 6-column field.
    #[test]
    fn time_and_size_forms() {
        assert_eq!(time(120), "120 ms");
        assert_eq!(time(999), "999 ms");
        assert_eq!(time(2300), "2.3 s");
        assert_eq!(time(9960), "10 s");
        assert_eq!(time(12_300), "12 s");
        assert_eq!(time(123_400), "123 s");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1000), "1.0 KB");
        assert_eq!(size(1229), "1.2 KB");
        assert_eq!(size(12_600), "12 KB");
        assert_eq!(size(126_000), "123 KB");
        assert_eq!(size(1000 * 1024), "1.0 MB");
        assert_eq!(size(1_023_488), "1.0 MB");
        assert_eq!(size(14 * 1024 * 1024), "14 MB");
        assert_eq!(size(1000 * 1024 * 1024), "1.0 GB");
        for ms in (0..1_000_000).step_by(7) {
            assert!(cells(&time(ms)) <= NUM_COL, "{ms}: {}", time(ms));
        }
        for bytes in (0..1000 * 1024 * 1024).step_by(997) {
            assert!(cells(&size(bytes)) <= NUM_COL, "{bytes}: {}", size(bytes));
        }
    }

    /// Final review M5: a control character in a space, folder, request,
    /// environment, selector or option name shows as `?`, so it takes the
    /// one line it is measured to and sends the terminal nothing.
    #[test]
    fn control_characters_in_names_show_as_question_marks() {
        let odd = "a\nb\tc\u{1b}[31m";
        let shown = "a?b?c?[31m";
        let space = line_row(&Line::Space { name: odd.into() }, None, 0, Instant::now(), 0);
        assert_eq!(paint(&space, false), shown);
        let folder =
            line_row(&Line::Folder { depth: 1, name: odd.into() }, None, 0, Instant::now(), 0);
        assert_eq!(paint(&folder, false), format!("  {shown}/"));
        let lines = [request(1, Some(Method::Get), odd)];
        assert_eq!(name_column(&lines), cells(shown));
        let r = row(&lines[0], &RowState::Waiting, name_column(&lines));
        assert_eq!(paint(&r, false), format!("  · GET    {shown}"));
        let mut p = plan(vec![(odd.into(), odd.into())], 1);
        p.env = Some(odd.into());
        p.space = Some(odd.into());
        let head = paint(&header(&p), false);
        assert_eq!(head.matches(shown).count(), 4, "{head:?}");
        assert!(!head.chars().any(char::is_control), "{head:?}");
    }

    #[test]
    fn a_passed_row_reads_like_the_mockup() {
        let r = row(
            &request(2, Some(Method::Get), "Get user"),
            &RowState::Done(result(Some(200), None)),
            15,
        );
        assert_eq!(
            paint(&r, false),
            "    ✓ GET    Get user         200  120 ms   512 B  https://dev/users/42"
        );
    }

    #[test]
    fn status_tones() {
        for (status, tone) in
            [(204, Tone::Ok), (301, Tone::Redirect), (404, Tone::Bad), (500, Tone::Bad)]
        {
            let r = row(
                &request(1, Some(Method::Get), "x"),
                &RowState::Done(result(Some(status), None)),
                1,
            );
            let seg = r.iter().find(|s| s.text.trim() == status.to_string()).unwrap();
            assert_eq!(seg.tone, Some(tone), "{status}");
        }
    }

    #[test]
    fn a_refused_row_shows_its_message() {
        let mut res = result(None, Some("{{id}} has no value"));
        res.method = Some(Method::Post);
        let r = row(&request(1, Some(Method::Post), "List"), &RowState::Done(res), 4);
        assert_eq!(paint(&r, false), "  ! POST   List  {{id}} has no value");
        assert_eq!(r.last().unwrap().tone, Some(Tone::NotSent));
    }

    #[test]
    fn a_broken_file_has_a_blank_method() {
        let r = row(
            &request(1, None, "Broken"),
            &RowState::Done(result(None, Some("cannot read it"))),
            6,
        );
        assert_eq!(paint(&r, false), "  !        Broken  cannot read it");
    }

    #[test]
    fn a_multi_line_error_goes_under_when_final() {
        let state = RowState::Done(result(None, Some("a\nb")));
        let r = row(&request(1, Some(Method::Get), "L"), &state, 1);
        assert!(paint(&r, false).ends_with("  a"));
        assert!(!paint(&r, false).contains('b'));
        assert_eq!(error_under(&state, 1), ["      b"]);
        assert!(error_under(&RowState::Done(result(Some(200), None)), 1).is_empty());
    }

    #[test]
    fn waiting_and_skipped_rows_are_all_dim() {
        for state in [RowState::Waiting, RowState::Skipped] {
            let r = row(&request(1, Some(Method::Get), "Audit"), &state, 5);
            assert!(r.iter().all(|s| s.tone == Some(Tone::Dim)));
        }
        let r = row(&request(1, Some(Method::Get), "Audit"), &RowState::Skipped, 5);
        assert_eq!(paint(&r, false), "  – GET    Audit  skipped");
        let r = row(&request(1, Some(Method::Get), "Audit"), &RowState::Waiting, 5);
        assert_eq!(paint(&r, false), "  · GET    Audit");
    }

    #[test]
    fn running_row_spins_with_the_frame() {
        let now = Instant::now();
        let state = RowState::Running { since: now, url: None };
        let r = line_row(&request(1, Some(Method::Get), "Stats"), Some(&state), 5, now, 2);
        assert!(paint(&r, false).starts_with("  ⠹ GET"));
        assert_eq!(r[1].tone, Some(Tone::Spin));
        let r = line_row(&request(1, Some(Method::Get), "Stats"), Some(&state), 5, now, 12);
        assert!(paint(&r, false).starts_with("  ⠹ GET"));
    }

    #[test]
    fn a_running_row_shows_its_time() {
        let now = Instant::now();
        let state = RowState::Running { since: now - Duration::from_millis(2100), url: None };
        let r = line_row(&request(1, Some(Method::Put), "Update"), Some(&state), 6, now, 0);
        assert!(paint(&r, false).contains("   2.1 s"));
    }

    /// The URL of a running row starts in the column a finished row's does
    /// (spec §5.1 mockup).
    #[test]
    fn a_running_url_lines_up_with_a_finished_one() {
        let now = Instant::now();
        let url = "https://dev/users/42";
        let line = request(2, Some(Method::Get), "Get user");
        let running = RowState::Running { since: now, url: Some(url.into()) };
        let running = paint(&line_row(&line, Some(&running), 15, now, 0), false);
        let done = paint(&row(&line, &RowState::Done(result(Some(200), None)), 15), false);
        let col = |s: &str| cells(&s[..s.find(url).unwrap()]);
        assert_eq!(col(&running), col(&done));
    }

    #[test]
    fn space_and_folder_lines() {
        let s = line_row(&Line::Space { name: "main".into() }, None, 0, Instant::now(), 0);
        assert_eq!(s, [toned("main", Tone::Space)]);
        let f =
            line_row(&Line::Folder { depth: 2, name: "users".into() }, None, 0, Instant::now(), 0);
        assert_eq!(paint(&f, false), "    users/");
        assert_eq!(f[1].tone, Some(Tone::Folder));
    }

    #[test]
    fn name_column_is_the_widest_name_capped() {
        let lines = [
            Line::Space { name: "a very long space name indeed, longer than any request".into() },
            request(1, None, "Get user"),
            request(1, None, "名前"),
        ];
        assert_eq!(name_column(&lines), 8);
        assert_eq!(name_column(&[request(1, None, &"n".repeat(50))]), 32);
        assert_eq!(name_column(&[]), 0);
    }

    #[test]
    fn fit_never_exceeds_the_width() {
        for w in [20, 30, 40, 80] {
            let mut r = long_row();
            fit(&mut r, w);
            assert!(width(&r) <= w, "{w}: {}", paint(&r, false));
        }
        for w in [0, 1, 2, 5] {
            let mut r = long_row();
            fit(&mut r, w);
            assert!(width(&r) <= w, "{w}");
        }
    }

    #[test]
    fn fit_clips_the_url_before_the_name() {
        let mut r = long_row();
        let full = width(&r);
        fit(&mut r, full - 10);
        let text = paint(&r, false);
        assert!(text.contains("A rather long request name here"), "{text}");
        assert!(text.ends_with('…'), "{text}");
        assert_eq!(width(&r), full - 10);
    }

    #[test]
    fn fit_clips_the_name_when_the_url_is_gone() {
        let mut r = long_row();
        // 34 columns of icon, method, status, time and size, then the name.
        fit(&mut r, 40);
        let text = paint(&r, false);
        assert!(text.contains("A rat…"), "{text}");
        assert!(!text.ends_with(' '), "{text}");
        assert!(!text.contains("https"), "{text}");
        assert_eq!(width(&r), 40);
    }

    #[test]
    fn fit_leaves_a_row_that_fits() {
        let mut r = long_row();
        let before = r.clone();
        fit(&mut r, 200);
        assert_eq!(r, before);
    }

    #[test]
    fn fit_measures_display_width() {
        let name = "名前のリクエスト";
        assert_eq!(name.len(), 24);
        assert_eq!(name.chars().count(), 8);
        let r =
            row(&request(1, Some(Method::Get), name), &RowState::Done(result(Some(200), None)), 16);
        assert_eq!(cells(&r[5].text), 16);
        let mut r = r;
        fit(&mut r, 30);
        assert!(width(&r) <= 30);
        // Whole characters only: 7 columns hold three wide ones and the `…`.
        assert_eq!(clip(name, 7), "名前の…");
        assert_eq!(clip(name, 8), "名前の…");
        assert_eq!(clip("🎉🎉🎉", 4), "🎉…");
    }

    #[test]
    fn a_name_past_the_cap_is_clipped_to_its_column() {
        let r = row(
            &request(1, Some(Method::Get), "abcdefghij"),
            &RowState::Done(result(Some(200), None)),
            6,
        );
        assert_eq!(r[5].text, "abcde…");
    }

    #[test]
    fn no_color_has_no_escapes() {
        assert!(!paint(&header(&plan(Vec::new(), 1)), false).contains('\x1b'));
        assert!(!paint(&long_row(), false).contains('\x1b'));
    }

    #[test]
    fn color_wraps_each_toned_segment() {
        let s = [Seg { text: "200".into(), tone: Some(Tone::Ok), shrink: 0 }];
        assert_eq!(paint(&s, true), "\x1b[32m200\x1b[0m");
        let both = [plain("a"), toned("b", Tone::Space)];
        assert_eq!(paint(&both, true), "a\x1b[1;34mb\x1b[0m");
    }

    #[test]
    fn header_lists_changed_selectors_and_jobs() {
        let p = plan(vec![("customer".into(), "globex".into())], 3);
        assert_eq!(
            paint(&header(&p), false),
            "postui suite  env Dev  space all  customer globex · 9 requests · 3 at a time"
        );
        assert_eq!(
            paint(&header(&plan(Vec::new(), 1)), false),
            "postui suite  env Dev  space all · 9 requests · one at a time"
        );
        let mut p = plan(Vec::new(), 1);
        p.env = None;
        p.space = Some("main".into());
        assert_eq!(
            paint(&header(&p), false),
            "postui suite  env (none)  space main · 9 requests · one at a time"
        );
    }

    #[test]
    fn progress_line() {
        assert_eq!(
            paint(&progress(5, 9, 3, 2, Duration::from_millis(2400)), false),
            " 5 of 9 · 3 passed · 2 failed · 2.4 s"
        );
    }

    #[test]
    fn totals_for_nothing() {
        assert_eq!(
            paint(&totals(Badge::Ok, 0, 0, 0, 0, Duration::ZERO), false),
            "  OK   0 requests · 0 passed · 0 failed · 0 ms"
        );
    }

    #[test]
    fn cancelled_totals_count_skipped() {
        let t = paint(&totals(Badge::Cancelled, 9, 4, 1, 4, Duration::from_secs(3)), false);
        assert!(t.contains(" CANCELLED "), "{t}");
        assert!(t.contains("· 4 skipped"), "{t}");
        let t = paint(&totals(Badge::Fail, 9, 7, 2, 0, Duration::from_secs(3)), false);
        assert!(t.starts_with("  FAIL   9 requests"), "{t}");
        assert!(!t.contains("skipped"));
    }

    #[test]
    fn sgr_table() {
        assert_eq!(sgr(Tone::Method(Method::Patch)), "33");
        assert_eq!(sgr(Tone::Method(Method::Options)), "90");
        assert_eq!(sgr(Tone::BadgeFail), "1;97;41");
    }
}
