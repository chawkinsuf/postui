//! `postui <command>`: the project's contents on the command line, for
//! scripts and agents (spec 2026-10-28 §4). The first argument picks the
//! command (`config::parse_cli` routes it here); [`parse`] reads the rest,
//! [`run`] runs it and prints its report as text, or as one JSON document
//! with `--json` (R17). Read commands open the project read-only, so they
//! write nothing (R16).

mod check;
pub mod mask;
pub mod out;
mod read;
mod run;
mod suite;
pub mod target;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

pub use run::RunOpts;

/// The `--timeout` a send gets when none is given.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The options every project command takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Global {
    /// `-p`: a directory, or a registered project's name; `None` is the
    /// current directory.
    pub project: Option<String>,
    /// `--json`: print one JSON document instead of text.
    pub json: bool,
}

/// A parsed command line, one variant per command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Projects {
        json: bool,
    },
    Show(Global),
    Requests {
        g: Global,
        space: Option<String>,
    },
    Vars {
        g: Global,
        env: Option<String>,
        show_secrets: bool,
        /// `vars <name>`: one variable or selector in full.
        name: Option<String>,
    },
    Check {
        g: Global,
        env: Option<String>,
    },
    Init {
        dir: PathBuf,
        name: Option<String>,
        json: bool,
    },
    Run {
        g: Global,
        request: String,
        opts: RunOpts,
        jq: Option<String>,
        output: Option<PathBuf>,
        dry_run: bool,
    },
    Suite {
        g: Global,
        space: Option<String>,
        opts: RunOpts,
        output: Option<PathBuf>,
    },
}

/// What [`parse`] makes of a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Run(Command),
    /// `help <cmd>` or `<cmd> --help`: this usage text goes to stdout.
    Help(&'static str),
}

/// The first arguments that are commands rather than a directory to open
/// (R18: a directory of that name opens as `postui ./show`).
pub const SUBCOMMANDS: &[&str] =
    &["projects", "show", "requests", "vars", "check", "init", "run", "suite", "help"];

/// Why a command could not give its report: the error envelope's
/// `error` and, when it is about one file, `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub file: Option<PathBuf>,
}

impl CliError {
    /// An error about no one file.
    pub fn new(message: impl Into<String>) -> Self {
        CliError { message: message.into(), file: None }
    }

    /// An error about `file`.
    pub fn at(message: impl Into<String>, file: PathBuf) -> Self {
        CliError { message: message.into(), file: Some(file) }
    }
}

/// The `-p` lines every project command's usage carries.
macro_rules! project_flag {
    () => {
        "  -p, --project <dir|name>  the project: a directory, or a registered
                            project's name (default: the current directory)
"
    };
}

/// The `--json` line every command's usage carries.
macro_rules! json_flag {
    () => {
        "  --json                    print one JSON document instead of text
"
    };
}

const PROJECTS_USAGE: &str = concat!(
    "\
usage: postui projects [--json]

The projects postui knows: each one's name, root, and whether the root is
still there; and the config directory they are listed in.

",
    json_flag!()
);

const SHOW_USAGE: &str = concat!(
    "\
usage: postui show [-p <dir|name>] [--json]

The project's name, root and files, its environments and spaces, and the
warnings opening it gave. Paths are relative to the root.

",
    project_flag!(),
    json_flag!()
);

const REQUESTS_USAGE: &str = concat!(
    "\
usage: postui requests [-p <dir|name>] [--space <space>] [--json]

The project's requests in sidebar order: each one's slug, method, URL as
written, name, and file (relative to the root).

",
    project_flag!(),
    "  --space <space>           only this space's requests
",
    json_flag!()
);

const VARS_USAGE: &str = concat!(
    "\
usage: postui vars [<name>] [-p <dir|name>] [--env <env>] [--show-secrets] [--json]

Every variable's value in an environment and where it comes from, and
every selector with its options' names and the one selected. With <name>,
that one variable or selector in full: a selector's options with their
values, and the file they are in. Secrets are masked.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
  --show-secrets            print secrets' values
",
    json_flag!()
);

const CHECK_USAGE: &str = concat!(
    "\
usage: postui check [-p <dir|name>] [--env <env>] [--json]

Parses and validates every project file, then resolves every request's
variables against an environment. Exits 1 when it finds an error.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
",
    json_flag!()
);

const INIT_USAGE: &str = concat!(
    "\
usage: postui init <dir> [--name <name>] [--json]

Starts a project in <dir> with the fewest files one needs. Never
overwrites a file.

  --name <name>             the project's name (default: the directory's)
",
    json_flag!()
);

const RUN_USAGE: &str = concat!(
    "\
usage: postui run <request> [-p <dir|name>] [--env <env>] [--select <sel>=<opt>]...
                   [--jq <filter>] [-o <file>] [--timeout <secs>] [--dry-run]
                   [--show-secrets] [--json]

Sends one request and prints what came back: its status, timings, headers
and body. <request> is a slug as `postui requests` lists it. Nothing is
written to the project. Each secret's exact value is masked in everything
printed; a value the server or a --jq filter transforms is not. Exits 1 for
a status of 400 or more, when the request can't be prepared or sent, or when
--jq fails.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
  --select <sel>=<opt>      pick an option of a selector for this run only;
                            can be given more than once
  --jq <filter>             print the filter's results instead of the body
  -o, --output <file>       write the body (or each jq result) to <file>
  --timeout <secs>          give up if the response hasn't started after this
                            long (connect, upload and redirects included),
                            or the body stalls this long between reads
                            (default: 30; 0: never)
  --dry-run                 print the request that would be sent; send nothing
  --show-secrets            print secrets' values
",
    json_flag!()
);

const SUITE_USAGE: &str = concat!(
    "\
usage: postui suite [-p <dir|name>] [--space <space>] [--env <env>]
                    [--select <sel>=<opt>]... [-o <file>] [--timeout <secs>]
                    [--show-secrets] [--json]

Sends every request of the project (or of one space), one at a time in
sidebar order, and prints each one's status and time, then the totals. A
request that can't be read or prepared is reported and the suite goes on.
Nothing is written to the project. Each secret's exact value is masked in
everything printed; a value the server transforms is not. Exits 1 when any
request failed to prepare or send or returned 400 or more.

",
    project_flag!(),
    "  --space <space>           only this space's requests
  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
  --select <sel>=<opt>      pick an option of a selector for this run only;
                            can be given more than once
  -o, --output <file>       also write what is printed to <file>
  --timeout <secs>          give up on a request if its response hasn't
                            started after this long (connect, upload and
                            redirects included), or the body stalls this
                            long between reads (default: 30; 0: never)
  --show-secrets            print secrets' values
",
    json_flag!()
);

/// One command: its name, the options it takes (by their long spelling;
/// `-p` is `--project`, `-o` is `--output`), what positional argument it
/// takes, and its usage.
struct Spec {
    name: &'static str,
    flags: &'static [&'static str],
    positional: Option<Positional>,
    usage: &'static str,
}

/// A command's positional argument.
#[derive(Clone, Copy, PartialEq)]
enum Positional {
    /// `init`'s directory (`~` expands).
    Dir,
    /// `run`'s request slug.
    Request,
    /// `vars`' variable or selector name.
    Name,
}

impl Positional {
    /// The noun the user reads.
    fn noun(self) -> &'static str {
        match self {
            Positional::Dir => "directory",
            Positional::Request => "request",
            Positional::Name => "name",
        }
    }
}

#[rustfmt::skip]
const SPECS: [Spec; 8] = [
    Spec { name: "projects", flags: &["--json"],                                                                                                   positional: None,                      usage: PROJECTS_USAGE },
    Spec { name: "show",     flags: &["--project", "--json"],                                                                                      positional: None,                      usage: SHOW_USAGE },
    Spec { name: "requests", flags: &["--project", "--json", "--space"],                                                                           positional: None,                      usage: REQUESTS_USAGE },
    Spec { name: "vars",     flags: &["--project", "--json", "--env", "--show-secrets"],                                                           positional: Some(Positional::Name),    usage: VARS_USAGE },
    Spec { name: "check",    flags: &["--project", "--json", "--env"],                                                                             positional: None,                      usage: CHECK_USAGE },
    Spec { name: "init",     flags: &["--json", "--name"],                                                                                         positional: Some(Positional::Dir),     usage: INIT_USAGE },
    Spec { name: "run",      flags: &["--project", "--json", "--env", "--select", "--show-secrets", "--timeout", "--jq", "--output", "--dry-run"], positional: Some(Positional::Request), usage: RUN_USAGE },
    Spec { name: "suite",    flags: &["--project", "--json", "--space", "--env", "--select", "--show-secrets", "--timeout", "--output"],           positional: None,                      usage: SUITE_USAGE },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// The usage a mistake on `name`'s line is told with: the command's own,
/// or the general one for `help` (whose mistake is naming no command).
pub fn usage(name: &str) -> &'static str {
    spec(name).map_or(crate::config::USAGE, |s| s.usage)
}

/// Parses a command line that starts at the command's name (one of
/// [`SUBCOMMANDS`]). `--help`/`-h` anywhere on an otherwise valid line, or
/// `help <cmd>`, asks for that command's usage; a mistake (an unknown
/// option, a value missing or given twice) is the `Err`, and it wins over
/// a help flag, as in `config::parse_cli`. Option values must be UTF-8;
/// `init`'s directory is kept as the OS gave it.
pub fn parse(args: &[OsString]) -> Result<Parsed, String> {
    let Some((name, rest)) = args.split_first() else {
        return Err("no command given".into());
    };
    let is_help = |a: &&OsString| *a == "--help" || *a == "-h";
    let wants_help = rest.iter().any(|a| is_help(&a));
    let rest: Vec<&OsString> = rest.iter().filter(|a| !is_help(a)).collect();
    let name = name.to_string_lossy();
    if name == "help" {
        return match rest.as_slice() {
            [] => Ok(Parsed::Help(crate::config::USAGE)),
            [cmd] => {
                let cmd = cmd.to_string_lossy();
                spec(&cmd).map(|s| Parsed::Help(s.usage)).ok_or(format!("no command '{cmd}'"))
            }
            [_, extra, ..] => Err(format!("unexpected argument '{}'", extra.to_string_lossy())),
        };
    }
    let spec = spec(&name).ok_or(format!("no command '{name}'"))?;
    let options = scan(spec, &rest)?;
    if wants_help {
        return Ok(Parsed::Help(spec.usage));
    }
    // Only `run` and `suite` take these options; the rest ignore the value.
    let opts = run_opts(&options)?;
    let g = Global { project: options.project, json: options.json };
    let positional =
        |what: &str| options.positional.clone().ok_or(format!("{} needs a {what}", spec.name));
    Ok(Parsed::Run(match spec.name {
        "projects" => Command::Projects { json: g.json },
        "show" => Command::Show(g),
        "requests" => Command::Requests { g, space: options.space },
        "vars" => Command::Vars {
            g,
            env: options.env,
            show_secrets: options.show_secrets,
            name: match &options.positional {
                Some(name) => Some(name.to_str().ok_or("the name is not UTF-8")?.to_string()),
                None => None,
            },
        },
        "check" => Command::Check { g, env: options.env },
        "init" => Command::Init {
            dir: PathBuf::from(positional("directory")?),
            name: options.name,
            json: g.json,
        },
        "suite" => Command::Suite {
            g,
            space: options.space,
            opts,
            output: options.output.map(PathBuf::from),
        },
        _ => {
            let request = positional("request")?;
            let request = request.to_str().ok_or("the request's slug is not UTF-8")?.to_string();
            let output = options.output.map(PathBuf::from);
            if options.dry_run && output.is_some() {
                return Err("-o needs a send; it can't go with --dry-run".into());
            }
            if options.dry_run && options.jq.is_some() {
                return Err("--jq needs a send; it can't go with --dry-run".into());
            }
            Command::Run { g, request, opts, jq: options.jq, output, dry_run: options.dry_run }
        }
    }))
}

/// The options `run` and `suite` share, with `--timeout` read: whole
/// seconds, `0` for none, 30 when not given.
fn run_opts(o: &Options) -> Result<RunOpts, String> {
    let timeout = match o.timeout.as_deref() {
        None => Some(DEFAULT_TIMEOUT),
        Some(secs) => match secs.parse::<u64>() {
            Ok(0) => None,
            Ok(n) => Some(Duration::from_secs(n)),
            Err(_) => {
                return Err(format!("--timeout takes a whole number of seconds, not '{secs}'"));
            }
        },
    };
    Ok(RunOpts {
        env: o.env.clone(),
        selects: o.selects.clone(),
        show_secrets: o.show_secrets,
        timeout,
    })
}

/// Every option a command line can carry, before the command picks its own.
#[derive(Default)]
struct Options {
    project: Option<String>,
    json: bool,
    space: Option<String>,
    env: Option<String>,
    show_secrets: bool,
    name: Option<String>,
    /// The command's one positional argument, as the OS gave it
    /// (`init`'s directory, `run`'s request, `vars`' name).
    positional: Option<OsString>,
    selects: Vec<(String, String)>,
    jq: Option<String>,
    output: Option<String>,
    timeout: Option<String>,
    dry_run: bool,
}

/// Reads `args` (help flags already out) against what `spec` takes.
/// A value comes as the next argument or after `=` (`--space=main`).
fn scan(spec: &Spec, args: &[&OsString]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if !text.starts_with('-') {
            let Some(what) = spec.positional else {
                return Err(format!("unexpected argument '{text}'"));
            };
            if o.positional.is_some() {
                return Err(format!("one {} at most; '{text}' is a second", what.noun()));
            }
            o.positional = Some(match arg.to_str() {
                Some(s) if what == Positional::Dir => {
                    crate::config::expand_tilde(s).into_os_string()
                }
                _ => (*arg).clone(),
            });
            continue;
        }
        let (spelled, inline) = match text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (&*text, None),
        };
        let flag = match spelled {
            "-p" => "--project",
            "-o" => "--output",
            other => other,
        };
        if !spec.flags.contains(&flag) {
            return Err(format!("unknown option '{text}'"));
        }
        let switch = match flag {
            "--json" => Some(&mut o.json),
            "--show-secrets" => Some(&mut o.show_secrets),
            "--dry-run" => Some(&mut o.dry_run),
            _ => None,
        };
        if let Some(on) = switch {
            if inline.is_some() {
                return Err(format!("{flag} takes no value"));
            }
            if *on {
                return Err(format!("{flag} is given twice"));
            }
            *on = true;
            continue;
        }
        let value = match inline {
            Some(_) if arg.to_str().is_none() => None,
            Some(value) => Some(value.to_string()),
            None => {
                let next = args.next().ok_or(format!("{spelled} needs a value"))?;
                next.to_str().map(str::to_string)
            }
        }
        .ok_or(format!("{spelled}'s value is not UTF-8"))?;
        let slot = match flag {
            // The one option that repeats, once per selector.
            "--select" => {
                let (name, option) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--select takes SELECTOR=OPTION, not '{value}'"))?;
                if o.selects.iter().any(|(s, _)| s == name) {
                    return Err(format!("--select {name} is given twice"));
                }
                o.selects.push((name.to_string(), option.to_string()));
                continue;
            }
            "--project" => &mut o.project,
            "--space" => &mut o.space,
            "--env" => &mut o.env,
            "--jq" => &mut o.jq,
            "--output" => &mut o.output,
            "--timeout" => &mut o.timeout,
            _ => &mut o.name,
        };
        if slot.is_some() {
            return Err(format!("{spelled} is given twice"));
        }
        *slot = Some(value);
    }
    Ok(o)
}

/// Runs `cmd`, printing its report (or the error envelope), and gives the
/// status to exit with: 0 on success, 1 when `check` finds an error or a
/// `run` fails, 2 on an error.
pub async fn run(cmd: Command) -> ExitCode {
    match cmd {
        Command::Projects { json } => finish(read::projects(), json, read::ProjectsReport::text),
        Command::Show(g) => finish(read::show(&g), g.json, read::ShowReport::text),
        Command::Requests { g, space } => {
            finish(read::requests(&g, space.as_deref()), g.json, read::RequestsReport::text)
        }
        Command::Vars { g, env, show_secrets, name: None } => {
            finish(read::vars(&g, env.as_deref(), show_secrets), g.json, read::VarsReport::text)
        }
        Command::Vars { g, env, show_secrets, name: Some(name) } => finish(
            read::var(&g, env.as_deref(), show_secrets, &name),
            g.json,
            read::VarReport::text,
        ),
        Command::Check { g, env } => match check::check(&g, env.as_deref()) {
            Ok(report) => {
                out::emit(&report, g.json, check::CheckReport::text).unwrap_or(report.status())
            }
            Err(e) => out::fail(&e, g.json),
        },
        Command::Init { dir, name, json } => {
            finish(read::init(&dir, name.as_deref()), json, read::InitReport::text)
        }
        Command::Run { g, request, opts, jq, output, dry_run } => {
            match run::run(&g, &request, &opts, jq.as_deref(), output.as_deref(), dry_run).await {
                Ok(report) => {
                    let printed = out::emit(&report, g.json, run::RunReport::text);
                    sent_status(printed, report.status())
                }
                Err(e) => out::fail(&e, g.json),
            }
        }
        Command::Suite { g, space, opts, output } => {
            match suite::suite(&g, space.as_deref(), &opts).await {
                Ok((mut report, mask)) => {
                    // The file is named (made absolute once) before
                    // rendering, so the report in it and on stdout are the
                    // same bytes.
                    let file = output.as_deref().map(run::absolute);
                    if let Some(Ok(path)) = &file {
                        report.set_output_file(path.clone());
                    }
                    let mut printed = match out::render(&report, g.json, suite::SuiteReport::text) {
                        Ok(printed) => printed,
                        Err(e) => return out::fail(&e, g.json),
                    };
                    // Written before printing, so stdout can say whether it
                    // worked. The requests are sent, so a failed write is a
                    // warning on the one report, not an error envelope.
                    if let Some(Err(e)) = file
                        .map(|path| path.and_then(|p| run::write_output(&p, printed.as_bytes())))
                    {
                        report.note_write_failure(mask.apply(&e.message));
                        printed = match out::render(&report, g.json, suite::SuiteReport::text) {
                            Ok(printed) => printed,
                            Err(e) => return out::fail(&e, g.json),
                        };
                    }
                    sent_status(crate::cli_out::print_out(&printed), report.status())
                }
                Err(e) => out::fail(&e, g.json),
            }
        }
    }
}

/// The status a `run` or `suite` ends with, given what printing its report
/// gave (`None` when it all went out) and the report's own `status`. A
/// reader that stopped (a closed pipe, which printing calls a success)
/// doesn't make a failed run succeed; printing that failed otherwise wins.
fn sent_status(printed: Option<ExitCode>, status: ExitCode) -> ExitCode {
    match printed {
        Some(code) if code != ExitCode::SUCCESS => code,
        _ => status,
    }
}

/// Prints a command's report, or its error.
fn finish<T: serde::Serialize>(
    result: Result<T, CliError>,
    json: bool,
    render_text: impl FnOnce(&T) -> String,
) -> ExitCode {
    match result {
        Ok(report) => out::emit(&report, json, render_text).unwrap_or(ExitCode::SUCCESS),
        Err(e) => out::fail(&e, json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Parsed, String> {
        parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    fn command(args: &[&str]) -> Command {
        match parsed(args) {
            Ok(Parsed::Run(c)) => c,
            other => panic!("{args:?} parsed as {other:?}"),
        }
    }

    fn help(args: &[&str]) -> &'static str {
        match parsed(args) {
            Ok(Parsed::Help(t)) => t,
            other => panic!("{args:?} parsed as {other:?}"),
        }
    }

    #[test]
    fn show_takes_the_project_and_json() {
        assert_eq!(
            command(&["show", "-p", "x", "--json"]),
            Command::Show(Global { project: Some("x".into()), json: true })
        );
        assert_eq!(
            command(&["show", "--project=x"]),
            Command::Show(Global { project: Some("x".into()), json: false })
        );
    }

    /// R17: `--text` is gone; text is what a command prints unasked.
    #[test]
    fn text_is_the_default_and_takes_no_flag() {
        assert_eq!(parsed(&["show", "--text"]).unwrap_err(), "unknown option '--text'");
    }

    #[test]
    fn requests_takes_a_space() {
        assert_eq!(
            command(&["requests", "--space", "main"]),
            Command::Requests {
                g: Global { project: None, json: false },
                space: Some("main".into())
            }
        );
    }

    #[test]
    fn vars_check_and_init_take_their_options() {
        assert_eq!(
            command(&["vars", "--env", "dev", "--show-secrets"]),
            Command::Vars {
                g: Global::default(),
                env: Some("dev".into()),
                show_secrets: true,
                name: None
            }
        );
        assert_eq!(
            command(&["vars", "user", "--json"]),
            Command::Vars {
                g: Global { project: None, json: true },
                env: None,
                show_secrets: false,
                name: Some("user".into())
            }
        );
        assert_eq!(
            command(&["check", "--env=dev", "-p", "api"]),
            Command::Check {
                g: Global { project: Some("api".into()), json: false },
                env: Some("dev".into())
            }
        );
        assert_eq!(
            command(&["init", "kit", "--name", "Kit"]),
            Command::Init { dir: PathBuf::from("kit"), name: Some("Kit".into()), json: false }
        );
        assert_eq!(command(&["projects"]), Command::Projects { json: false });
    }

    #[test]
    fn a_selector_is_selected_once() {
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(
            err(&["run", "a", "--select", "user=bob", "--select", "user=alice"]),
            "--select user is given twice"
        );
        assert_eq!(
            err(&["suite", "--select", "user=bob", "--select", "user=bob"]),
            "--select user is given twice"
        );
    }

    #[test]
    fn run_takes_a_request_and_its_options() {
        assert_eq!(
            command(&[
                "run",
                "main/a",
                "--env",
                "dev",
                "--select",
                "user=bob",
                "--select",
                "org=x",
                "--timeout",
                "0",
                "--dry-run"
            ]),
            Command::Run {
                g: Global::default(),
                request: "main/a".into(),
                opts: RunOpts {
                    env: Some("dev".into()),
                    selects: vec![("user".into(), "bob".into()), ("org".into(), "x".into())],
                    show_secrets: false,
                    timeout: None,
                },
                jq: None,
                output: None,
                dry_run: true,
            }
        );
        assert_eq!(
            command(&[
                "run",
                "a",
                "-p",
                "api",
                "--jq",
                ".id",
                "-o",
                "out.bin",
                "--show-secrets",
                "--timeout=5",
                "--json"
            ]),
            Command::Run {
                g: Global { project: Some("api".into()), json: true },
                request: "a".into(),
                opts: RunOpts {
                    env: None,
                    selects: vec![],
                    show_secrets: true,
                    timeout: Some(Duration::from_secs(5)),
                },
                jq: Some(".id".into()),
                output: Some(PathBuf::from("out.bin")),
                dry_run: false,
            }
        );
    }

    #[test]
    fn run_sends_with_a_thirty_second_timeout_by_default() {
        match command(&["run", "a"]) {
            Command::Run { opts, .. } => assert_eq!(opts.timeout, Some(Duration::from_secs(30))),
            other => panic!("{other:?}"),
        }
    }

    /// A closed stdout doesn't turn a failed run into a success; a write
    /// error otherwise is the status, and a full print keeps the run's.
    #[test]
    fn a_failed_run_keeps_its_status_whatever_printing_gave() {
        let failed = ExitCode::from(1);
        let closed_pipe = Some(ExitCode::SUCCESS);
        assert_eq!(sent_status(closed_pipe, failed), failed);
        assert_eq!(sent_status(closed_pipe, ExitCode::SUCCESS), ExitCode::SUCCESS);
        assert_eq!(sent_status(None, failed), failed);
        assert_eq!(sent_status(Some(ExitCode::from(2)), ExitCode::SUCCESS), ExitCode::from(2));
    }

    #[test]
    fn run_mistakes_say_what_is_wrong() {
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(err(&["run"]), "run needs a request");
        assert_eq!(err(&["run", "a", "b"]), "one request at most; 'b' is a second");
        assert!(err(&["run", "a", "--timeout", "soon"]).contains("--timeout"));
        assert!(err(&["run", "a", "--timeout", "-1"]).contains("--timeout"));
        assert!(err(&["run", "a", "-o", "out.bin", "--dry-run"]).contains("-o needs a send"));
        assert!(err(&["run", "a", "--jq", ".id", "--dry-run"]).contains("--jq needs a send"));
        assert_eq!(err(&["run", "a", "--env", "x", "--env", "y"]), "--env is given twice");
        assert_eq!(err(&["run", "a", "--space", "x"]), "unknown option '--space'");
        assert_eq!(err(&["show", "--select", "a=b"]), "unknown option '--select'");
    }

    #[test]
    fn an_unknown_option_is_named() {
        let err = parsed(&["show", "--bogus"]).unwrap_err();
        assert!(err.contains("--bogus"), "{err}");
    }

    /// Each mistake says what is wrong, and a help flag doesn't hide it.
    #[test]
    fn mistakes_say_what_is_wrong() {
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(err(&["projects", "--space", "x"]), "unknown option '--space'");
        assert_eq!(err(&["show", "-p"]), "-p needs a value");
        assert_eq!(err(&["show", "-p", "a", "--project", "b"]), "--project is given twice");
        assert_eq!(err(&["show", "--json", "--json"]), "--json is given twice");
        assert_eq!(err(&["show", "--json=yes"]), "--json takes no value");
        assert_eq!(err(&["show", "api"]), "unexpected argument 'api'");
        assert_eq!(err(&["vars", "a", "b"]), "one name at most; 'b' is a second");
        assert_eq!(err(&["init"]), "init needs a directory");
        assert_eq!(err(&["init", "a", "b"]), "one directory at most; 'b' is a second");
        assert_eq!(err(&["show", "--bogus", "-h"]), "unknown option '--bogus'");
        assert_eq!(err(&["help", "bogus"]), "no command 'bogus'");
    }

    #[test]
    fn help_is_the_commands_usage() {
        assert!(help(&["help", "vars"]).starts_with("usage: postui vars"));
        assert!(help(&["vars", "-h"]).starts_with("usage: postui vars"));
        // A command's help needs none of its required arguments.
        assert!(help(&["init", "--help"]).starts_with("usage: postui init"));
        assert_eq!(help(&["help"]), crate::config::USAGE);
    }

    /// Every command but `help` has a usage, and only those do.
    #[test]
    fn every_command_has_a_usage() {
        let named: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        assert_eq!(named, SUBCOMMANDS[..SUBCOMMANDS.len() - 1]);
        for spec in &SPECS {
            assert!(spec.usage.starts_with(&format!("usage: postui {} ", spec.name)));
        }
    }
}
