//! The `-i` picker: a small form for the environment, space, selector
//! options and how many requests go at once, drawn in place and run before
//! the suite (spec 2026-11-13 §7). [`Form`] is the model, apart from the
//! terminal, so its keys are tested as text; [`pick`] is the raw-mode loop
//! around it. Nothing the form chooses is written: the project is opened
//! read-only, and the picks only become the run's `--env`, `--select`s,
//! `--space` and `--jobs`.

use super::live::{put, wipe};
use super::paint::{Seg, Tone, fit, paint, printable, width};
use crate::cli::read::{displayed, env_load_error, match_env, no_such_space, open};
use crate::cli::run::RunOpts;
use crate::cli::{CliError, Global, target};
use crate::ui::cells;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use postui_core::varmodel::{self, EnvData};
use std::io::Write;

/// What a selector with no selection shows as its first value.
const NONE: &str = "(none)";
/// At once offers 1 to this, then what `--jobs` gave if larger.
const AT_ONCE: usize = 16;

/// A selector's options in one environment (or all of them, if shared) and
/// the option saved for it.
pub(super) struct SelectorChoices {
    pub name: String,
    pub options: Vec<String>,
    pub saved: Option<String>,
}

/// An environment and the non-shared selectors that have options in it, in
/// the model's order.
pub(super) struct EnvChoices {
    pub slug: String,
    pub name: String,
    pub selectors: Vec<SelectorChoices>,
}

/// Everything the form offers, read from the project once.
pub(super) struct Catalog {
    pub project: String,
    pub envs: Vec<EnvChoices>,
    pub shared: Vec<SelectorChoices>,
    /// (slug, display name) of each space, in project order.
    pub spaces: Vec<(String, String)>,
    /// How many requests each space has, in the same order.
    pub counts: Vec<usize>,
    /// The index in `envs` of the one the project opens on.
    pub active_env: Option<usize>,
}

/// `selector`'s option names in `env`, if it has any there.
fn options_in(model: &varmodel::VarModel, env: &EnvData, selector: &str) -> Option<Vec<String>> {
    varmodel::options_of(model, env, selector)
        .map(|options| options.keys().cloned().collect::<Vec<_>>())
        .filter(|options| !options.is_empty())
}

impl Catalog {
    /// Reads the project `g` names, read-only: each environment loaded in
    /// turn for its selectors' options, with the saved selections.
    pub(super) fn read(g: &Global) -> Result<Catalog, CliError> {
        let root = target::resolve(g.project.as_deref())?;
        // Its warnings are the run's to print, at its finish.
        let (mut project, _) = open(&root)?;
        let slugs = project.environments().to_vec();
        let mut envs = Vec::new();
        for slug in &slugs {
            let data =
                project.load_environment(slug).map_err(|e| env_load_error(&project, slug, e))?;
            let model = project.variables();
            let saved = project.selections_for(slug);
            let selectors = model
                .selectors
                .iter()
                .filter(|(_, decl)| !decl.shared)
                .filter_map(|(name, _)| {
                    Some(SelectorChoices {
                        name: name.clone(),
                        options: options_in(model, &data, name)?,
                        saved: saved.get(name).cloned(),
                    })
                })
                .collect();
            envs.push(EnvChoices { slug: slug.clone(), name: project.env_name(slug), selectors });
        }
        let model = project.variables();
        let none = EnvData::default();
        let shared = model
            .selectors
            .iter()
            .filter(|(_, decl)| decl.shared)
            .filter_map(|(name, _)| {
                Some(SelectorChoices {
                    name: name.clone(),
                    options: options_in(model, &none, name)?,
                    saved: project.local().shared_selections.get(name).cloned(),
                })
            })
            .collect();
        let spaces: Vec<(String, String)> =
            project.spaces().iter().map(|s| (s.clone(), project.space_name(s))).collect();
        let counts = spaces.iter().map(|(slug, _)| displayed(&project, slug).len()).collect();
        let active_env =
            project.active_env().and_then(|active| slugs.iter().position(|s| s == active));
        Ok(Catalog { project: project.display_name(), envs, shared, spaces, counts, active_env })
    }
}

/// A key the form answers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Cancel,
    Interrupt,
}

/// How the form ended.
#[derive(Debug)]
pub(super) enum Done {
    Run,
    Cancel,
    Interrupt,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Env,
    Space,
    Selector,
    Jobs,
}

/// One row: its label, its values and which one is showing.
struct Row {
    label: String,
    kind: Kind,
    values: Vec<String>,
    at: usize,
    /// `values[0]` is `(none)`: the selector has no selection.
    none: bool,
}

impl Row {
    fn plain(label: &str, kind: Kind, values: Vec<String>, at: usize) -> Row {
        Row { label: label.to_string(), kind, values, at, none: false }
    }

    /// The selector's row: its option from `selects` if the environment has
    /// that option, else the saved one; with neither, `(none)` comes first.
    fn selector(choice: &SelectorChoices, selects: &[(String, String)]) -> Row {
        let has = |option: &&String| choice.options.contains(option);
        let given = selects.iter().find(|(name, _)| *name == choice.name).map(|(_, o)| o);
        let pick = given.filter(has).or(choice.saved.as_ref().filter(has));
        let mut values = choice.options.clone();
        let (at, none) = match pick.and_then(|p| values.iter().position(|v| v == p)) {
            Some(at) => (at, false),
            None => {
                values.insert(0, NONE.to_string());
                (0, true)
            }
        };
        Row { label: choice.name.clone(), kind: Kind::Selector, values, at, none }
    }

    fn unset(&self) -> bool {
        self.none && self.at == 0
    }
}

/// The picker's model: rows, the cursor and the starting `--select`s.
pub(super) struct Form {
    catalog: Catalog,
    selects: Vec<(String, String)>,
    rows: Vec<Row>,
    cursor: usize,
    /// How many of the rows after Space are the environment's own selectors.
    env_rows: usize,
}

impl Form {
    /// The form for `catalog`, starting at `opts.env` (a slug or a name, as
    /// `--env` takes it) or the project's own, `space` (a slug) or all, the
    /// selections `opts.selects` gives or the saved ones, and `jobs`.
    pub(super) fn new(
        catalog: Catalog,
        space: Option<&str>,
        opts: &RunOpts,
        jobs: usize,
    ) -> Result<Form, CliError> {
        let env_at = match opts.env.as_deref() {
            Some(wanted) => Some(find_env(&catalog, wanted)?),
            None => catalog.active_env.filter(|&at| at < catalog.envs.len()).or(
                // The first, when none is open.
                (!catalog.envs.is_empty()).then_some(0),
            ),
        };
        let space_at = match space {
            None => 0,
            Some(wanted) => {
                1 + catalog
                    .spaces
                    .iter()
                    .position(|(slug, _)| slug == wanted)
                    .ok_or_else(|| no_such_space(wanted))?
            }
        };
        let mut rows = Vec::new();
        if !catalog.envs.is_empty() {
            let names = catalog.envs.iter().map(|e| e.name.clone()).collect();
            rows.push(Row::plain("Environment", Kind::Env, names, env_at.unwrap_or(0)));
        }
        let mut spaces = vec!["all".to_string()];
        spaces.extend(catalog.spaces.iter().map(|(_, name)| name.clone()));
        rows.push(Row::plain("Space", Kind::Space, spaces, space_at));
        let own = env_at.map_or(&[][..], |at| &catalog.envs[at].selectors[..]);
        let env_rows = own.len();
        rows.extend(own.iter().map(|c| Row::selector(c, &opts.selects)));
        rows.extend(catalog.shared.iter().map(|c| Row::selector(c, &opts.selects)));
        let mut counts: Vec<String> = (1..=AT_ONCE).map(|n| n.to_string()).collect();
        if jobs > AT_ONCE {
            counts.push(jobs.to_string());
        }
        let at = if jobs > AT_ONCE { AT_ONCE } else { jobs.max(1) - 1 };
        rows.push(Row::plain("At once", Kind::Jobs, counts, at));
        Ok(Form { catalog, selects: opts.selects.clone(), rows, cursor: 0, env_rows })
    }

    /// The row the Space row is, and the first selector row after it.
    fn space_row(&self) -> usize {
        usize::from(!self.catalog.envs.is_empty())
    }

    /// Answers `key`; `Some` when the form is over.
    pub(super) fn press(&mut self, key: Key) -> Option<Done> {
        match key {
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down => self.cursor = (self.cursor + 1).min(self.rows.len() - 1),
            Key::Left => self.step(false),
            Key::Right => self.step(true),
            Key::Enter => return Some(Done::Run),
            Key::Cancel => return Some(Done::Cancel),
            Key::Interrupt => return Some(Done::Interrupt),
        }
        None
    }

    /// Steps the cursor row's value, wrapping; a new environment rebuilds
    /// its selector rows from what is saved there (and the `--select`s that
    /// fit), while the shared rows keep their picks.
    fn step(&mut self, forward: bool) {
        let row = &mut self.rows[self.cursor];
        let n = row.values.len();
        let before = row.at;
        row.at = if forward { (row.at + 1) % n } else { (row.at + n - 1) % n };
        if row.kind == Kind::Env && row.at != before {
            let first = self.space_row() + 1;
            let env = &self.catalog.envs[self.rows[0].at];
            let own: Vec<Row> =
                env.selectors.iter().map(|c| Row::selector(c, &self.selects)).collect();
            let old = first..first + self.env_rows;
            self.env_rows = own.len();
            self.rows.splice(old, own);
        }
    }

    /// The form as styled lines (spec §7) for a terminal of `cols` × `rows`,
    /// drawn as the live band is (final review I2). Each line is fitted to
    /// the width so it takes one line: the key line keeps the hints that fit
    /// whole, the project's name clips, and a row gives up its value's
    /// padding, then the value, then the label. There are no more lines than
    /// the height less one, so the next draw can reach the first (as R13):
    /// the blank lines go first, then the count line, the key line and the
    /// title; then only the rows up to the cursor's show.
    pub(super) fn lines(&self, cols: usize, rows: usize) -> Vec<Vec<Seg>> {
        let seg = |text: &str, tone, shrink| Seg { text: text.to_string(), tone, shrink };
        let title = vec![
            seg("postui suite", Some(Tone::Bold), 0),
            seg("  ", None, 0),
            seg(&printable(&self.catalog.project), Some(Tone::Dim), 1),
        ];
        let label_col = self.rows.iter().map(|r| cells(&printable(&r.label))).max().unwrap_or(0);
        let mut form: Vec<Vec<Seg>> = self
            .rows
            .iter()
            .enumerate()
            .map(|(at, row)| {
                let values: Vec<String> = row.values.iter().map(|v| printable(v)).collect();
                let widest = values.iter().map(|v| cells(v)).max().unwrap_or(0);
                let value = &values[row.at];
                let here = at == self.cursor;
                vec![
                    seg(if here { "▸ " } else { "  " }, None, 0),
                    seg(&pad(&printable(&row.label), label_col), Some(Tone::Dim), 3),
                    seg("  ‹ ", None, 0),
                    seg(value, here.then_some(Tone::Space), 2),
                    seg(&pad("", widest - cells(value)), None, 1),
                    seg(" ›", None, 0),
                ]
            })
            .collect();
        let space = &self.rows[self.space_row()];
        let n = match space.at {
            0 => self.catalog.counts.iter().sum(),
            at => self.catalog.counts.get(at - 1).copied().unwrap_or(0),
        };
        let count = vec![seg(&format!("  {n} requests"), Some(Tone::Dim), 0)];
        let hints = [("↑↓", "move"), ("←→", "change"), ("enter", "run"), ("esc", "cancel")];
        // As many hints as fit, whole; with not even one, the first, clipped.
        let keys = (1..=hints.len())
            .rev()
            .map(|n| key_line(&hints[..n]))
            .find(|line| width(line) <= cols)
            .unwrap_or_else(|| key_line(&hints[..1]));
        // What shows besides the rows: the blank lines, the count line, the
        // key line and the title, given up in that order (as R13) while the
        // form is taller than the room.
        let room = rows.saturating_sub(1).max(1);
        let mut shown = [true; 4];
        let others = |shown: &[bool; 4]| {
            3 * usize::from(shown[0]) + shown[1..].iter().filter(|kept| **kept).count()
        };
        for at in 0..shown.len() {
            if form.len() + others(&shown) > room {
                shown[at] = false;
            }
        }
        let [blank, count_kept, keys_kept, title_kept] = shown;
        // Still too many rows: those up to the cursor's, as many as fit.
        let fits = room.saturating_sub(others(&shown)).max(1);
        let first = (self.cursor + 1).saturating_sub(fits);
        form.truncate(first + fits);
        form.drain(..first);
        let mut lines = Vec::new();
        if title_kept {
            lines.push(title);
        }
        let gap = || blank.then(Vec::new);
        lines.extend(gap());
        lines.append(&mut form);
        lines.extend(gap());
        if count_kept {
            lines.push(count);
        }
        lines.extend(gap());
        if keys_kept {
            lines.push(keys);
        }
        for line in &mut lines {
            fit(line, cols);
        }
        lines
    }

    /// What the form chose: the space (`None` for all), `opts` with the
    /// environment (when the project has any) and the selections replaced,
    /// and how many go at once.
    pub(super) fn picks(&self, opts: &RunOpts) -> (Option<String>, RunOpts, usize) {
        let space = match self.rows[self.space_row()].at {
            0 => None,
            at => Some(self.catalog.spaces[at - 1].0.clone()),
        };
        let mut run = opts.clone();
        if !self.catalog.envs.is_empty() {
            run.env = Some(self.catalog.envs[self.rows[0].at].slug.clone());
        }
        run.selects = self
            .rows
            .iter()
            .filter(|row| row.kind == Kind::Selector && !row.unset())
            .map(|row| (row.label.clone(), row.values[row.at].clone()))
            .collect();
        let jobs = self.rows.last().and_then(|row| row.values[row.at].parse().ok()).unwrap_or(1);
        (space, run, jobs)
    }
}

/// `text` padded with spaces to `cols` display columns.
fn pad(text: &str, cols: usize) -> String {
    format!("{text}{}", " ".repeat(cols.saturating_sub(cells(text))))
}

/// The key line with `hints`: each key bold, what it does dim and the
/// first to clip.
fn key_line(hints: &[(&str, &str)]) -> Vec<Seg> {
    let mut line = vec![Seg { text: " ".into(), tone: None, shrink: 0 }];
    for (at, (key, what)) in hints.iter().enumerate() {
        line.push(Seg { text: format!(" {key} "), tone: Some(Tone::Bold), shrink: 0 });
        let gap = if at + 1 < hints.len() { " " } else { "" };
        line.push(Seg { text: format!("{what}{gap}"), tone: Some(Tone::Dim), shrink: 1 });
    }
    line
}

/// The index of the environment `wanted` names, matched as `--env` is.
fn find_env(catalog: &Catalog, wanted: &str) -> Result<usize, CliError> {
    let envs: Vec<(&str, &str)> =
        catalog.envs.iter().map(|e| (e.slug.as_str(), e.name.as_str())).collect();
    match_env(&envs, wanted)
}

/// What [`pick`] came to.
pub(in crate::cli) enum Picked {
    /// Run with these, as if given on the command line.
    Run { space: Option<String>, opts: RunOpts, jobs: usize },
    /// Nothing is sent: the exit status (1 for Esc or `q`, 130 for Ctrl+C).
    Cancelled(u8),
}

/// The key a terminal event is, if it is one the form answers: arrows and
/// `k j h l`, Enter, Esc or `q`, Ctrl+C.
fn key_of(key: KeyEvent) -> Option<Key> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Key::Interrupt);
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Key::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Key::Down),
        KeyCode::Left | KeyCode::Char('h') => Some(Key::Left),
        KeyCode::Right | KeyCode::Char('l') => Some(Key::Right),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc | KeyCode::Char('q') => Some(Key::Cancel),
        _ => None,
    }
}

/// Raw mode and a hidden cursor for as long as it lives, so a panic or an
/// early return gives the terminal back.
struct RawGuard;

impl RawGuard {
    fn new() -> Result<RawGuard, CliError> {
        enable_raw_mode()
            .map_err(|e| CliError::new(format!("could not read keys from the terminal: {e}")))?;
        let mut out = std::io::stdout();
        let _ = queue!(out, Hide);
        let _ = out.flush();
        Ok(RawGuard)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        let _ = queue!(out, Show);
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

/// Shows the form where the cursor is and reads keys until it is over, then
/// clears it, so the run starts where the form was (spec §7). `color` is
/// false under `NO_COLOR`.
pub(in crate::cli) async fn pick(
    g: &Global,
    space: Option<&str>,
    opts: &RunOpts,
    jobs: usize,
    color: bool,
) -> Result<Picked, CliError> {
    let mut form = Form::new(Catalog::read(g)?, space, opts, jobs)?;
    let _raw = RawGuard::new()?;
    let mut events = EventStream::new();
    let mut out = std::io::stdout();
    let mut drawn = 0;
    let done = loop {
        // The form's old lines wiped a line at a time, as the live band's
        // are (see `wipe`), then the form again, fitted to the terminal as
        // it is now (80×24 if it can't say).
        let mut buf = Vec::new();
        wipe(&mut buf, drawn);
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let lines = form.lines(usize::from(cols), usize::from(rows));
        drawn = lines.len();
        for line in &lines {
            put(&mut buf, &paint(line, color));
        }
        let _ = out.write_all(&buf);
        let _ = out.flush();
        let key = loop {
            match events.next().await {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    if let Some(key) = key_of(key) {
                        break Some(key);
                    }
                }
                // A resize: the form is the same, drawn again to the new
                // size.
                Some(Ok(Event::Resize(..))) => break None,
                Some(Ok(_)) => {}
                // The terminal is gone: nothing to pick with.
                Some(Err(_)) | None => break Some(Key::Cancel),
            }
        };
        if let Some(done) = key.and_then(|key| form.press(key)) {
            break done;
        }
    };
    let mut buf = Vec::new();
    wipe(&mut buf, drawn);
    let _ = out.write_all(&buf);
    let _ = out.flush();
    Ok(match done {
        Done::Run => {
            let (space, opts, jobs) = form.picks(opts);
            Picked::Run { space, opts, jobs }
        }
        Done::Cancel => Picked::Cancelled(1),
        Done::Interrupt => Picked::Cancelled(super::INTERRUPTED),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sel(name: &str, options: &[&str], saved: Option<&str>) -> SelectorChoices {
        SelectorChoices {
            name: name.into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            saved: saved.map(str::to_string),
        }
    }

    /// dev (active): customer acme|globex, saved acme. prod: region eu|us,
    /// nothing saved. Shared: locale en|fr, saved fr. Spaces main (6) and
    /// Admin (3).
    fn catalog() -> Catalog {
        Catalog {
            project: "my-api".into(),
            envs: vec![
                EnvChoices {
                    slug: "dev".into(),
                    name: "Dev".into(),
                    selectors: vec![sel("customer", &["acme", "globex"], Some("acme"))],
                },
                EnvChoices {
                    slug: "prod".into(),
                    name: "Prod".into(),
                    selectors: vec![sel("region", &["eu", "us"], None)],
                },
            ],
            shared: vec![sel("locale", &["en", "fr"], Some("fr"))],
            spaces: vec![("main".into(), "main".into()), ("admin".into(), "Admin".into())],
            counts: vec![6, 3],
            active_env: Some(0),
        }
    }

    fn opts(env: Option<&str>, selects: &[(&str, &str)]) -> RunOpts {
        RunOpts {
            env: env.map(str::to_string),
            selects: selects.iter().map(|(s, o)| (s.to_string(), o.to_string())).collect(),
            vars: vec![("v".into(), "1".into())],
            show_secrets: true,
            timeout: Some(Duration::from_secs(7)),
        }
    }

    fn form() -> Form {
        Form::new(catalog(), None, &opts(None, &[]), 1).unwrap()
    }

    /// The plain text of every line on a terminal of `cols` × `rows`.
    fn text_in(form: &Form, cols: usize, rows: usize) -> Vec<String> {
        form.lines(cols, rows).iter().map(|l| paint(l, false)).collect()
    }

    /// The plain text of every line, on a terminal with room for it all.
    fn text(form: &Form) -> Vec<String> {
        text_in(form, 200, 100)
    }

    /// The row labelled `label`'s value: what is between `‹ ` and ` ›`.
    fn value(form: &Form, label: &str) -> String {
        let after_marker = |l: &str| l.chars().skip(2).collect::<String>();
        let line = text(form).into_iter().find(|l| after_marker(l).starts_with(label)).unwrap();
        let rest = &line[line.find("‹ ").unwrap() + "‹ ".len()..];
        rest.trim_end_matches('›').trim_end().to_string()
    }

    fn presses(form: &mut Form, keys: &[Key]) {
        for key in keys {
            form.press(*key);
        }
    }

    /// Spec 2026-11-13 §7: the saved environment and selections start the
    /// form, and a `--select` overrides the saved option.
    #[test]
    fn starts_from_saved_values_and_flags() {
        let plain = form();
        assert_eq!(value(&plain, "Environment"), "Dev");
        assert_eq!(value(&plain, "customer"), "acme");
        assert_eq!(value(&plain, "locale"), "fr");
        assert_eq!(value(&plain, "At once"), "1");
        let flagged =
            Form::new(catalog(), None, &opts(None, &[("customer", "globex")]), 1).unwrap();
        assert_eq!(value(&flagged, "customer"), "globex");
        // An option the environment doesn't have falls back to the saved one.
        let missing = Form::new(catalog(), None, &opts(None, &[("customer", "nope")]), 1).unwrap();
        assert_eq!(value(&missing, "customer"), "acme");
        // `--env` by slug or by name, as `pick_env` matches them.
        for given in ["prod", "Prod", "PROD"] {
            let form = Form::new(catalog(), None, &opts(Some(given), &[]), 1).unwrap();
            assert_eq!(value(&form, "Environment"), "Prod", "{given}");
        }
        assert!(Form::new(catalog(), None, &opts(Some("qa"), &[]), 1).is_err());
        let admin = Form::new(catalog(), Some("admin"), &opts(None, &[]), 1).unwrap();
        assert_eq!(value(&admin, "Space"), "Admin");
        assert!(Form::new(catalog(), Some("nope"), &opts(None, &[]), 1).is_err());
    }

    /// The picker matches `--env` and `--space` through the plain run's own
    /// code, so the errors are the plain run's words (spec §7).
    #[test]
    fn unknown_env_and_space_say_what_the_plain_run_says() {
        let message = |r: Result<Form, CliError>| r.err().unwrap().message;
        assert_eq!(
            message(Form::new(catalog(), None, &opts(Some("qa"), &[]), 1)),
            "no such environment \"qa\"; the environments are dev, prod"
        );
        assert_eq!(
            message(Form::new(catalog(), Some("nope"), &opts(None, &[]), 1)),
            "no such space \"nope\" in this project"
        );
        let mut bare = catalog();
        bare.envs.clear();
        assert_eq!(
            message(Form::new(bare, None, &opts(Some("qa"), &[]), 1)),
            "no such environment \"qa\"; this project has none"
        );
    }

    #[test]
    fn left_right_wrap_and_up_down_stop() {
        let mut form = form();
        // Up on the first row stays on it.
        form.press(Key::Up);
        assert!(text(&form)[2].starts_with("▸ Environment"));
        // Right on Dev (of two) goes to Prod, and wraps back to Dev.
        form.press(Key::Right);
        assert_eq!(value(&form, "Environment"), "Prod");
        form.press(Key::Right);
        assert_eq!(value(&form, "Environment"), "Dev");
        form.press(Key::Left);
        assert_eq!(value(&form, "Environment"), "Prod");
        // Down stops on the last row, At once.
        presses(&mut form, &[Key::Down; 9]);
        assert!(text(&form).iter().any(|l| l.starts_with("▸ At once")));
        // Past the last option wraps to the first.
        form.press(Key::Left);
        assert_eq!(value(&form, "At once"), "16");
        form.press(Key::Right);
        assert_eq!(value(&form, "At once"), "1");
    }

    #[test]
    fn changing_env_rebuilds_its_selectors_and_keeps_shared() {
        let mut form = form();
        // Shared pick moved to en first.
        presses(&mut form, &[Key::Down, Key::Down, Key::Down, Key::Left]);
        assert_eq!(value(&form, "locale"), "en");
        assert_eq!(value(&form, "customer"), "acme");
        presses(&mut form, &[Key::Up, Key::Up, Key::Up, Key::Right]);
        assert_eq!(value(&form, "Environment"), "Prod");
        let rows = text(&form);
        assert!(rows.iter().all(|l| !l.contains("customer")), "{rows:#?}");
        // prod's region has nothing saved.
        assert_eq!(value(&form, "region"), "(none)");
        assert_eq!(value(&form, "locale"), "en");
        // Back to dev: its saved acme again.
        form.press(Key::Right);
        assert_eq!(value(&form, "customer"), "acme");
    }

    /// A `--select` is re-applied in an environment that has its option.
    #[test]
    fn changing_env_reapplies_flags_that_fit() {
        let flags = opts(None, &[("customer", "globex"), ("region", "us")]);
        let mut form = Form::new(catalog(), None, &flags, 1).unwrap();
        assert_eq!(value(&form, "customer"), "globex");
        form.press(Key::Right);
        assert_eq!(value(&form, "region"), "us");
    }

    #[test]
    fn an_unset_selector_offers_none_first() {
        let mut form = Form::new(catalog(), None, &opts(Some("prod"), &[]), 1).unwrap();
        presses(&mut form, &[Key::Down, Key::Down]);
        assert_eq!(value(&form, "region"), "(none)");
        let (_, run, _) = form.picks(&opts(None, &[]));
        assert_eq!(run.selects, [("locale".to_string(), "fr".to_string())]);
        form.press(Key::Right);
        assert_eq!(value(&form, "region"), "eu");
        form.press(Key::Left);
        assert_eq!(value(&form, "region"), "(none)");
        form.press(Key::Left);
        assert_eq!(value(&form, "region"), "us");
    }

    #[test]
    fn a_set_selector_has_no_none() {
        let mut form = form();
        presses(&mut form, &[Key::Down, Key::Down]);
        // acme, globex: two values; a third Right is back at acme.
        presses(&mut form, &[Key::Right, Key::Right]);
        assert_eq!(value(&form, "customer"), "acme");
        form.press(Key::Left);
        assert_eq!(value(&form, "customer"), "globex");
    }

    #[test]
    fn no_envs_no_environment_row() {
        let mut bare = catalog();
        bare.envs.clear();
        bare.active_env = None;
        let form = Form::new(bare, None, &opts(None, &[]), 1).unwrap();
        let rows = text(&form);
        assert_eq!(rows[2], "▸ Space    ‹ all   ›");
        assert!(rows.iter().all(|l| !l.contains("Environment")));
        // No environment to name: the run keeps what was given.
        let (_, run, _) = form.picks(&opts(None, &[]));
        assert_eq!(run.env, None);
        assert_eq!(run.selects, [("locale".to_string(), "fr".to_string())]);
    }

    #[test]
    fn at_once_offers_one_to_sixteen_and_a_bigger_flag() {
        let mut form = Form::new(catalog(), None, &opts(None, &[]), 20).unwrap();
        assert_eq!(value(&form, "At once"), "20");
        presses(&mut form, &[Key::Down; 9]);
        form.press(Key::Left);
        assert_eq!(value(&form, "At once"), "16");
        let seen: Vec<String> = (0..16)
            .map(|_| {
                form.press(Key::Left);
                value(&form, "At once")
            })
            .collect();
        assert_eq!(seen[..15], (1..=15).rev().map(|n| n.to_string()).collect::<Vec<_>>());
        assert_eq!(seen[15], "20");
        let small = Form::new(catalog(), None, &opts(None, &[]), 16).unwrap();
        assert_eq!(value(&small, "At once"), "16");
    }

    #[test]
    fn picks_become_run_opts() {
        let mut form = Form::new(catalog(), Some("admin"), &opts(Some("prod"), &[]), 1).unwrap();
        // region eu, jobs 4.
        presses(&mut form, &[Key::Down, Key::Down, Key::Right]);
        presses(&mut form, &[Key::Down, Key::Down, Key::Right, Key::Right, Key::Right]);
        let given = opts(None, &[("ignored", "x")]);
        let (space, run, jobs) = form.picks(&given);
        assert_eq!(space.as_deref(), Some("admin"));
        assert_eq!(run.env.as_deref(), Some("prod"));
        assert_eq!(
            run.selects,
            [("region".to_string(), "eu".to_string()), ("locale".to_string(), "fr".to_string())]
        );
        assert_eq!((&run.vars, run.show_secrets, run.timeout), (&given.vars, true, given.timeout));
        assert_eq!(jobs, 4);
        // All spaces is no space.
        assert_eq!(form_all().picks(&given).0, None);
    }

    fn form_all() -> Form {
        form()
    }

    #[test]
    fn enter_runs_esc_cancels() {
        assert!(matches!(form().press(Key::Enter), Some(Done::Run)));
        assert!(matches!(form().press(Key::Cancel), Some(Done::Cancel)));
        assert!(form().press(Key::Down).is_none());
    }

    #[test]
    fn ctrl_c_interrupts() {
        assert!(matches!(form().press(Key::Interrupt), Some(Done::Interrupt)));
    }

    #[test]
    fn request_count_follows_the_space_row() {
        let mut form = form();
        assert!(text(&form).contains(&"  9 requests".to_string()));
        // Environment, Space: the cursor is on Space after one Down.
        form.press(Key::Down);
        form.press(Key::Right);
        assert!(text(&form).contains(&"  6 requests".to_string()));
        form.press(Key::Right);
        assert!(text(&form).contains(&"  3 requests".to_string()));
    }

    /// The form's layout, with its cursor and key line (spec §7). The labels
    /// are padded to the widest of them, so a long selector name still
    /// lines its values up (final review I2).
    #[test]
    fn the_form_is_laid_out_as_the_mockup() {
        let mut form = form();
        form.press(Key::Down);
        assert_eq!(
            text(&form),
            [
                "postui suite  my-api",
                "",
                "  Environment  ‹ Dev  ›",
                "▸ Space        ‹ all   ›",
                "  customer     ‹ acme   ›",
                "  locale       ‹ fr ›",
                "  At once      ‹ 1  ›",
                "",
                "  9 requests",
                "",
                "  ↑↓ move  ←→ change  enter run  esc cancel",
            ]
        );
        let mut long = catalog();
        long.shared[0].name = "a-much-longer-selector".into();
        let rows = text(&Form::new(long, None, &opts(None, &[]), 1).unwrap());
        let at = |l: &String| cells(&l[..l.find('‹').unwrap()]);
        let columns: Vec<usize> = rows.iter().filter(|l| l.contains('‹')).map(at).collect();
        assert!(columns.iter().all(|&c| c == 2 + 22 + 2), "{rows:#?}");
    }

    /// The cursor's value is in `Space` tone, the label dim.
    #[test]
    fn the_cursor_value_is_toned() {
        let form = form();
        let lines = form.lines(200, 100);
        let line = &lines[2];
        let at = line.iter().find(|s| s.text.starts_with("Dev")).unwrap();
        assert_eq!(at.tone, Some(Tone::Space));
        assert_eq!(line[1].tone, Some(Tone::Dim));
        let other = &lines[3];
        assert!(other.iter().all(|s| s.tone != Some(Tone::Space)));
    }

    /// A catalog whose names are all long: the project, an environment, a
    /// selector and its option.
    fn long_catalog() -> Catalog {
        let mut c = catalog();
        c.project = "a project with a rather long name".into();
        c.envs[0].name = "Development, the shared one".into();
        c.envs[0].selectors[0].name = "customer-account".into();
        c.envs[0].selectors[0].options[0] = "acme-corporation-international".into();
        c
    }

    /// Final review I2: every line of the form is one terminal line however
    /// narrow the terminal (review focus 1, 20–40 columns), so the redraw's
    /// count of what it drew is right and nothing is left behind.
    #[test]
    fn the_form_fits_a_narrow_terminal() {
        for form in [form(), Form::new(long_catalog(), None, &opts(None, &[]), 1).unwrap()] {
            for cols in [20, 30, 38, 43, 80] {
                for line in text_in(&form, cols, 100) {
                    assert!(cells(&line) <= cols, "{cols}: {line:?}");
                }
            }
        }
        // The key line gives up whole hints from its end; the values clip
        // before the labels do.
        let narrow = text_in(&form(), 38, 100);
        assert_eq!(narrow[10], "  ↑↓ move  ←→ change  enter run");
        let long = Form::new(long_catalog(), None, &opts(None, &[]), 1).unwrap();
        let narrow = text_in(&long, 30, 100);
        assert_eq!(narrow[0], "postui suite  a project with…");
        // Padded to `customer-account`, the widest label.
        assert!(narrow[2].starts_with(&format!("▸ {}  ‹ D", pad("Environment", 16))));
        assert!(narrow[2].contains('…'), "{narrow:#?}");
    }

    /// Final review I2, as R13 for the band: the form never draws more lines
    /// than the terminal is high less one. The blank lines go first, then
    /// the count line, the key line and the title; then only the rows around
    /// the cursor's.
    #[test]
    fn a_short_terminal_drops_lines_and_keeps_the_cursor_row() {
        let form = form();
        // Five rows: eleven lines with everything.
        assert_eq!(text_in(&form, 80, 12).len(), 11);
        assert_eq!(
            text_in(&form, 80, 10),
            [
                "postui suite  my-api",
                "▸ Environment  ‹ Dev  ›",
                "  Space        ‹ all   ›",
                "  customer     ‹ acme   ›",
                "  locale       ‹ fr ›",
                "  At once      ‹ 1  ›",
                "  9 requests",
                "  ↑↓ move  ←→ change  enter run  esc cancel",
            ]
        );
        assert!(!text_in(&form, 80, 8).iter().any(|l| l.contains("requests")));
        assert!(text_in(&form, 80, 8).iter().any(|l| l.contains("esc cancel")));
        assert_eq!(text_in(&form, 80, 7)[0], "postui suite  my-api");
        assert!(!text_in(&form, 80, 7).iter().any(|l| l.contains("esc")));
        assert!(text_in(&form, 80, 6)[0].starts_with("▸ Environment"));
        let mut form = form;
        for rows in 1..=12 {
            for cursor in 0..5 {
                form.cursor = cursor;
                let lines = text_in(&form, 80, rows);
                assert!(lines.len() <= rows.saturating_sub(1).max(1), "{rows}: {lines:#?}");
                assert_eq!(
                    lines.iter().filter(|l| l.starts_with('▸')).count(),
                    1,
                    "{rows} rows, cursor {cursor}: {lines:#?}"
                );
            }
        }
        form.cursor = 4;
        assert_eq!(text_in(&form, 80, 3), ["  locale       ‹ fr ›", "▸ At once      ‹ 1  ›"]);
    }

    /// Final review M5: control characters in the names the form shows are
    /// `?`; the picks keep the names as they are.
    #[test]
    fn control_characters_in_names_show_as_question_marks() {
        let mut c = catalog();
        c.project = "my\napi".into();
        c.envs[0].name = "D\tev".into();
        c.shared[0].name = "lo\u{1b}cale".into();
        let form = Form::new(c, None, &opts(None, &[]), 1).unwrap();
        let rows = text(&form);
        assert_eq!(rows[0], "postui suite  my?api");
        assert!(rows.iter().any(|l| l.contains("‹ D?ev")), "{rows:#?}");
        assert!(rows.iter().any(|l| l.starts_with("  lo?cale")), "{rows:#?}");
        assert!(rows.iter().all(|l| !l.chars().any(char::is_control)), "{rows:#?}");
        let (_, run, _) = form.picks(&opts(None, &[]));
        assert!(run.selects.contains(&("lo\u{1b}cale".to_string(), "fr".to_string())));
    }

    /// The catalog read from a project: each environment's own selectors with
    /// options there (a selector with none in an environment is left out),
    /// the shared ones, the saved picks, the spaces with their request
    /// counts, and the environment the project opens on.
    #[test]
    fn the_catalog_is_read_from_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let put = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put(
            "project.toml",
            "name = \"Shop\"\nspaces = [\"main\", \"admin\"]\nenvironments = [\"dev\", \"prod\"]\n\n\
             [space.admin]\nname = \"Admin\"\n\n[environment.prod]\nname = \"Production\"\n",
        );
        put(
            "variables.toml",
            "[base]\n\n[selectors.customer]\nfields = [\"c\"]\n\n[selectors.region]\n\
             fields = [\"r\"]\n\n[selectors.locale]\nfields = [\"l\"]\nshared = true\n\n\
             [options.locale.en]\nl = \"en\"\n\n[options.locale.fr]\nl = \"fr\"\n",
        );
        put(
            "environments/dev.toml",
            "base = \"d\"\n\n[options.customer.acme]\nc = \"a\"\n\n[options.customer.globex]\nc = \"g\"\n",
        );
        put("environments/prod.toml", "base = \"p\"\n\n[options.region.eu]\nr = \"e\"\n");
        put(
            ".postui/state.toml",
            "environment = \"prod\"\n\n[selections.dev]\ncustomer = \"globex\"\n",
        );
        for slug in ["main/a", "main/b", "admin/c"] {
            put(&format!("requests/{slug}.toml"), "method = \"GET\"\nurl = \"http://x\"\n");
        }
        let g = Global { project: Some(dir.path().to_str().unwrap().into()), json: false };
        let catalog = Catalog::read(&g).unwrap();
        assert_eq!(catalog.project, "Shop");
        assert_eq!(catalog.active_env, Some(1));
        let envs: Vec<(&str, &str)> =
            catalog.envs.iter().map(|e| (e.slug.as_str(), e.name.as_str())).collect();
        assert_eq!(envs, [("dev", "dev"), ("prod", "Production")]);
        let dev = &catalog.envs[0].selectors;
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].name, "customer");
        assert_eq!(dev[0].options, ["acme", "globex"]);
        assert_eq!(dev[0].saved.as_deref(), Some("globex"));
        let prod = &catalog.envs[1].selectors;
        assert_eq!(
            (prod[0].name.as_str(), prod[0].options.clone()),
            ("region", vec!["eu".to_string()])
        );
        assert_eq!(prod[0].saved, None);
        assert_eq!(catalog.shared.len(), 1);
        assert_eq!(catalog.shared[0].options, ["en", "fr"]);
        assert_eq!(catalog.shared[0].saved, None);
        assert_eq!(
            catalog.spaces,
            [("main".to_string(), "main".to_string()), ("admin".to_string(), "Admin".to_string())]
        );
        assert_eq!(catalog.counts, [2, 1]);
    }

    #[test]
    fn keys_map_from_terminal_events() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code, mods| key_of(KeyEvent::new(code, mods));
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Up, none), Some(Key::Up));
        assert_eq!(key(KeyCode::Char('k'), none), Some(Key::Up));
        assert_eq!(key(KeyCode::Char('j'), none), Some(Key::Down));
        assert_eq!(key(KeyCode::Char('h'), none), Some(Key::Left));
        assert_eq!(key(KeyCode::Char('l'), none), Some(Key::Right));
        assert_eq!(key(KeyCode::Enter, none), Some(Key::Enter));
        assert_eq!(key(KeyCode::Esc, none), Some(Key::Cancel));
        assert_eq!(key(KeyCode::Char('q'), none), Some(Key::Cancel));
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), Some(Key::Interrupt));
        assert_eq!(key(KeyCode::Char('c'), none), None);
        assert_eq!(key(KeyCode::Char('x'), none), None);
    }
}
