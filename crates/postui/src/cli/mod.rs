//! `postui <command>`: the project's contents on the command line, for
//! scripts and agents (spec 2026-10-28 §4). The first argument picks the
//! command (`config::parse_cli` routes it here); [`parse`] reads the rest,
//! [`run`] runs it and prints its report as text, or as one JSON document
//! with `--json` (R17). Read commands open the project read-only, so they
//! write nothing (R16).

mod check;
mod edit;
pub mod mask;
pub mod out;
mod read;
mod run;
mod skill;
mod suite;
pub mod target;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

pub use run::{Filter, RunOpts, Shown};

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
        filter: Option<Filter>,
        output: Option<PathBuf>,
        dry_run: bool,
        shown: Shown,
    },
    Suite {
        g: Global,
        space: Option<String>,
        opts: RunOpts,
        output: Option<PathBuf>,
    },
    EditBegin {
        g: Global,
        note: Option<String>,
    },
    EditEnd {
        g: Global,
        force: bool,
    },
    EditStatus(Global),
    SkillInstall {
        /// `--project`: into the current directory, not the user's home.
        into_project: bool,
        force: bool,
        json: bool,
    },
    SkillShow,
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
pub const SUBCOMMANDS: &[&str] = &[
    "projects", "show", "requests", "vars", "check", "init", "run", "suite", "edit", "skill",
    "help",
];

/// Why a command could not give its report: the error envelope's
/// `error` and, when it is about one file, `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
    pub file: Option<PathBuf>,
    /// A refusal (`edit begin` over a flag already up): the command did
    /// what it should, and the answer is no, so the run ends with 1 rather
    /// than an error's 2.
    pub refused: bool,
}

impl CliError {
    /// An error about no one file.
    pub fn new(message: impl Into<String>) -> Self {
        CliError { message: message.into(), file: None, refused: false }
    }

    /// An error about `file`.
    pub fn at(message: impl Into<String>, file: PathBuf) -> Self {
        CliError { message: message.into(), file: Some(file), refused: false }
    }

    /// A refusal about `file`.
    pub fn refusal(message: impl Into<String>, file: PathBuf) -> Self {
        CliError { refused: true, ..CliError::at(message, file) }
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
                   [--var <name>=<value>]... [--jq <filter> | --view[=<name>]]
                   [-o <file>] [-i | -v] [--timeout <secs>] [--dry-run]
                   [--show-secrets] [--json]

Sends one request and prints the response body, as curl does, but as text
(-o writes its exact bytes); -i adds the status and headers, -v the whole
run. A status of 400 or more, errors and warnings go to stderr. <request> is
a slug as `postui requests` lists it.
Nothing is written to the project. Each secret's exact value is masked in
everything printed; a value the server or a jq filter transforms is not.
Exits 1 for a status of 400 or more, when the request can't be prepared or
sent, or when the jq filter fails.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
  --select <sel>=<opt>      pick an option of a selector for this run only;
                            can be given more than once
  --var <name>=<value>      give a variable this value for this run only,
                            over every other source; can be given more than
                            once
  --jq <filter>             print the filter's results instead of the body
  --view[=<name>]           filter with the request's own jq filter (the one
                            its response pane applies), or its view <name>
  -o, --output <file>       write the body to <file>, as it came; with --jq or
                            --view, as text with secrets masked, and the
                            results still print
  -i, --include             print the status and response headers first
  -v, --verbose             print the whole run: the request sent, the
                            status, timings and headers, then the body
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
                    [--select <sel>=<opt>]... [--var <name>=<value>]...
                    [-o <file>] [--timeout <secs>] [--show-secrets] [--json]

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
  --var <name>=<value>      give a variable this value for this run only,
                            over every other source; can be given more than
                            once
  -o, --output <file>       also write what is printed to <file>
  --timeout <secs>          give up on a request if its response hasn't
                            started after this long (connect, upload and
                            redirects included), or the body stalls this
                            long between reads (default: 30; 0: never)
  --show-secrets            print secrets' values
",
    json_flag!()
);

const EDIT_USAGE: &str = "\
usage: postui edit begin|end|status [options]

An agent's edit of a project's files: edit begin before changing the TOML,
edit end after. An edit needs the project closed in postui; while one is in
progress postui won't open the project, and shows \"Updates in progress\"
until it ends. postui help edit <begin|end|status> gives each one's options.
";

const EDIT_BEGIN_USAGE: &str = concat!(
    "\
usage: postui edit begin [-p <dir|name>] [--note <text>] [--json]

Starts an edit of the project's files. Refuses when an edit is already in
progress, and while postui has the project open (ask the user to switch to
another project or quit postui, then retry). Exits 1 when it refuses.

",
    project_flag!(),
    "  --note <text>             what the edit is for; postui shows it
",
    json_flag!()
);

const EDIT_END_USAGE: &str = concat!(
    "\
usage: postui edit end [-p <dir|name>] [--force] [--json]

Checks the project as postui check does and, when it finds no error, ends
the edit, and postui can open the project again. Exits 1 when check finds
an error (the edit stays in progress, unless --force), or when no edit is in
progress (the user overrode it in postui).

",
    project_flag!(),
    "  --force                   end the edit even when check finds errors, or
                            its flag won't parse: for one the user confirms
                            is no longer running
",
    json_flag!()
);

const EDIT_STATUS_USAGE: &str = concat!(
    "\
usage: postui edit status [-p <dir|name>] [--json]

Whether an edit is in progress (since when, and its note), and whether
postui has the project open.

",
    project_flag!(),
    json_flag!()
);

const SKILL_USAGE: &str = "\
usage: postui skill install|show [options]

The Claude Code skill that teaches an agent postui's commands and project
files. postui help skill <install|show> gives each one's options.
";

const SKILL_INSTALL_USAGE: &str = concat!(
    "\
usage: postui skill install [--user | --project] [--force] [--json]

Installs the skill for Claude Code. A file already there that differs is
left alone and listed, and the run exits 1, unless --force.

  --user                    into ~/.claude/skills/postui (the default)
  --project                 into ./.claude/skills/postui, for this directory
  --force                   replace files that differ
",
    json_flag!()
);

const SKILL_SHOW_USAGE: &str = "\
usage: postui skill show

Prints the skill's SKILL.md.
";

/// A command spelled in two words (`edit begin`): the first, the second
/// words it takes, and the usage `help <first>` prints.
struct Group {
    name: &'static str,
    verbs: &'static [&'static str],
    usage: &'static str,
}

const GROUPS: [Group; 2] = [
    Group { name: "edit", verbs: &["begin", "end", "status"], usage: EDIT_USAGE },
    Group { name: "skill", verbs: &["install", "show"], usage: SKILL_USAGE },
];

fn group(name: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.name == name)
}

/// One command: its name (two words for a [`Group`]'s), the options it
/// takes (by their long spelling; `-p` is `--project`, `-o` is
/// `--output`), what positional argument it takes, and its usage.
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
const SPECS: [Spec; 13] = [
    Spec { name: "projects", flags: &["--json"],                                                                                                                                                positional: None,                      usage: PROJECTS_USAGE },
    Spec { name: "show",     flags: &["--project", "--json"],                                                                                                                                   positional: None,                      usage: SHOW_USAGE },
    Spec { name: "requests", flags: &["--project", "--json", "--space"],                                                                                                                        positional: None,                      usage: REQUESTS_USAGE },
    Spec { name: "vars",     flags: &["--project", "--json", "--env", "--show-secrets"],                                                                                                        positional: Some(Positional::Name),    usage: VARS_USAGE },
    Spec { name: "check",    flags: &["--project", "--json", "--env"],                                                                                                                          positional: None,                      usage: CHECK_USAGE },
    Spec { name: "init",     flags: &["--json", "--name"],                                                                                                                                      positional: Some(Positional::Dir),     usage: INIT_USAGE },
    Spec { name: "run",      flags: &["--project", "--json", "--env", "--select", "--var", "--show-secrets", "--timeout", "--jq", "--view", "--output", "--dry-run", "--include", "--verbose"], positional: Some(Positional::Request), usage: RUN_USAGE },
    Spec { name: "suite",    flags: &["--project", "--json", "--space", "--env", "--select", "--var", "--show-secrets", "--timeout", "--output"],                                               positional: None,                      usage: SUITE_USAGE },
    Spec { name: "edit begin",  flags: &["--project", "--json", "--note"],                                                                                                                      positional: None,                      usage: EDIT_BEGIN_USAGE },
    Spec { name: "edit end",    flags: &["--project", "--json", "--force"],                                                                                                                     positional: None,                      usage: EDIT_END_USAGE },
    Spec { name: "edit status", flags: &["--project", "--json"],                                                                                                                                positional: None,                      usage: EDIT_STATUS_USAGE },
    // `--project` is a switch here (into this directory), not `-p`'s project.
    Spec { name: "skill install", flags: &["--user", "--project", "--force", "--json"],                                                                                                         positional: None,                      usage: SKILL_INSTALL_USAGE },
    Spec { name: "skill show",    flags: &[],                                                                                                                                                   positional: None,                      usage: SKILL_SHOW_USAGE },
];

fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// The usage a mistake on `name`'s line is told with: the command's own, a
/// group's for its first word alone, or the general one for `help` (whose
/// mistake is naming no command).
pub fn usage(name: &str) -> &'static str {
    usage_named(name).unwrap_or(crate::config::USAGE)
}

/// The usage of a command (`edit begin`) or a group (`edit`) by its name.
fn usage_named(name: &str) -> Option<&'static str> {
    spec(name).map(|s| s.usage).or_else(|| group(name).map(|g| g.usage))
}

/// [`usage`] for a whole command line: a group's first two words name its
/// command, when the second is one of its verbs.
pub fn usage_of(args: &[OsString]) -> &'static str {
    let words: Vec<String> = args.iter().take(2).map(|a| a.to_string_lossy().into()).collect();
    match words.as_slice() {
        [first, verb, ..] if group(first).is_some_and(|g| g.verbs.contains(&verb.as_str())) => {
            usage(&format!("{first} {verb}"))
        }
        [first, ..] => usage(first),
        [] => crate::config::USAGE,
    }
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
        let words: Vec<_> = rest.iter().map(|a| a.to_string_lossy()).collect();
        let (cmd, extra) = match words.as_slice() {
            [] => return Ok(Parsed::Help(crate::config::USAGE)),
            // A group's verb, never an option after the group's name.
            [first, verb, extra @ ..] if group(first).is_some() && !verb.starts_with('-') => {
                (format!("{first} {verb}"), extra.first())
            }
            [cmd, extra @ ..] => (cmd.to_string(), extra.first()),
        };
        if let Some(extra) = extra {
            return Err(format!("unexpected argument '{extra}'"));
        }
        return usage_named(&cmd).map(Parsed::Help).ok_or(format!("no command '{cmd}'"));
    }
    // A group's command is its first two words.
    let (name, rest) = match group(&name) {
        None => (name.into_owned(), rest.as_slice()),
        Some(g) => match rest.split_first() {
            Some((verb, rest)) if !verb.to_string_lossy().starts_with('-') => {
                (format!("{name} {}", verb.to_string_lossy()), rest)
            }
            _ if wants_help => return Ok(Parsed::Help(g.usage)),
            _ => return Err(format!("{name} needs one of: {}", g.verbs.join(", "))),
        },
    };
    let spec = spec(&name).ok_or(format!("no command '{name}'"))?;
    let options = scan(spec, rest)?;
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
        "edit begin" => Command::EditBegin { g, note: options.note },
        "edit end" => Command::EditEnd { g, force: options.force },
        "edit status" => Command::EditStatus(g),
        "skill install" => {
            if options.user && options.into_project {
                return Err("--user and --project name two places; give one".into());
            }
            Command::SkillInstall {
                into_project: options.into_project,
                force: options.force,
                json: g.json,
            }
        }
        "skill show" => Command::SkillShow,
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
            if options.dry_run && options.view.is_some() {
                return Err("--view needs a send; it can't go with --dry-run".into());
            }
            if g.json && (options.include || options.verbose) {
                return Err("--json prints the whole run; -i and -v are for the text".into());
            }
            let filter = match (options.jq, options.view) {
                (Some(_), Some(_)) => {
                    return Err("--view and --jq both pick a filter; give one".into());
                }
                (Some(jq), None) => Some(Filter::Jq(jq)),
                (None, Some(view)) => Some(Filter::View(view)),
                (None, None) => None,
            };
            // `-v` shows everything `-i` does.
            let shown = match (options.verbose, options.include) {
                (true, _) => Shown::Verbose,
                (false, true) => Shown::Include,
                (false, false) => Shown::Body,
            };
            Command::Run { g, request, opts, filter, output, dry_run: options.dry_run, shown }
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
        vars: o.vars.clone(),
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
    vars: Vec<(String, String)>,
    jq: Option<String>,
    /// `--view` (`Some(None)`) or `--view=NAME`.
    view: Option<Option<String>>,
    output: Option<String>,
    timeout: Option<String>,
    dry_run: bool,
    include: bool,
    verbose: bool,
    note: Option<String>,
    force: bool,
    user: bool,
    into_project: bool,
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
            // `skill install`'s `--project` is a place, not a project `-p`
            // could name.
            "-p" if spec.name != "skill install" => "--project",
            "-o" => "--output",
            "-i" => "--include",
            "-v" => "--verbose",
            other => other,
        };
        if !spec.flags.contains(&flag) {
            return Err(format!("unknown option '{text}'"));
        }
        // A name only after `=`, so the request slug after a bare `--view`
        // is never read as one (as `--keydump[=BITS]`).
        if flag == "--view" {
            if o.view.is_some() {
                return Err("--view is given twice".into());
            }
            if inline == Some("") {
                return Err("--view= needs a view's name".into());
            }
            if inline.is_some() && arg.to_str().is_none() {
                return Err("--view's value is not UTF-8".into());
            }
            o.view = Some(inline.map(str::to_string));
            continue;
        }
        let switch = match flag {
            "--json" => Some(&mut o.json),
            "--show-secrets" => Some(&mut o.show_secrets),
            "--dry-run" => Some(&mut o.dry_run),
            "--include" => Some(&mut o.include),
            "--verbose" => Some(&mut o.verbose),
            "--force" => Some(&mut o.force),
            "--user" => Some(&mut o.user),
            // `skill install`'s place, where every other command's is a
            // project to name.
            "--project" if spec.name == "skill install" => Some(&mut o.into_project),
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
            "--var" => {
                let (name, var_value) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--var takes NAME=VALUE, not '{value}'"))?;
                if !postui_core::vars::is_valid_var_name(name) {
                    return Err(format!(
                        "--var: '{name}' is not a variable name (letters, digits, _ and -)"
                    ));
                }
                if o.vars.iter().any(|(n, _)| n == name) {
                    return Err(format!("--var {name} is given twice"));
                }
                o.vars.push((name.to_string(), var_value.to_string()));
                continue;
            }
            "--project" => &mut o.project,
            "--space" => &mut o.space,
            "--env" => &mut o.env,
            "--jq" => &mut o.jq,
            "--output" => &mut o.output,
            "--timeout" => &mut o.timeout,
            "--note" => &mut o.note,
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
        Command::Run { g, request, opts, filter, output, dry_run, shown } => {
            match run::run(&g, &request, &opts, filter.as_ref(), output.as_deref(), dry_run).await {
                Ok(report) => sent_status(report.print(g.json, shown), report.status()),
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
        Command::EditBegin { g, note } => {
            finish(edit::begin(&g, note.as_deref()), g.json, edit::BeginReport::text)
        }
        Command::EditEnd { g, force } => match edit::end(&g, force) {
            Ok(report) => {
                out::emit(&report, g.json, edit::EndReport::text).unwrap_or(report.status())
            }
            Err(e) => out::fail(&e, g.json),
        },
        Command::EditStatus(g) => finish(edit::status(&g), g.json, edit::StatusReport::text),
        Command::SkillInstall { into_project, force, json } => {
            match skill::install(into_project, force) {
                Ok(report) => {
                    out::emit(&report, json, skill::InstallReport::text).unwrap_or(report.status())
                }
                Err(e) => out::fail(&e, json),
            }
        }
        Command::SkillShow => {
            crate::cli_out::print_out(skill::FILES[0].1).unwrap_or(ExitCode::SUCCESS)
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
                    vars: vec![],
                    show_secrets: false,
                    timeout: None,
                },
                filter: None,
                output: None,
                dry_run: true,
                shown: Shown::Body,
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
                    vars: vec![],
                    show_secrets: true,
                    timeout: Some(Duration::from_secs(5)),
                },
                filter: Some(Filter::Jq(".id".into())),
                output: Some(PathBuf::from("out.bin")),
                dry_run: false,
                shown: Shown::Body,
            }
        );
    }

    fn run_parts(args: &[&str]) -> (RunOpts, Option<Filter>, Shown) {
        match command(args) {
            Command::Run { opts, filter, shown, .. } => (opts, filter, shown),
            other => panic!("{other:?}"),
        }
    }

    /// D3: bare `--view` is the pane's filter, and a name comes only after
    /// `=`, so a slug after `--view` stays the request.
    #[test]
    fn view_takes_a_name_only_after_equals() {
        assert_eq!(run_parts(&["run", "a", "--view"]).1, Some(Filter::View(None)));
        assert_eq!(
            run_parts(&["run", "a", "--view=fast"]).1,
            Some(Filter::View(Some("fast".into())))
        );
        match command(&["run", "--view", "main/a"]) {
            Command::Run { request, filter, .. } => {
                assert_eq!((request.as_str(), filter), ("main/a", Some(Filter::View(None))));
            }
            other => panic!("{other:?}"),
        }
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(err(&["run", "a", "--view", "--view"]), "--view is given twice");
        assert_eq!(
            err(&["run", "a", "--view", "--jq", ".id"]),
            "--view and --jq both pick a filter; give one"
        );
        assert!(err(&["run", "a", "--view", "--dry-run"]).contains("--view needs a send"));
        assert_eq!(err(&["run", "a", "--view="]), "--view= needs a view's name");
    }

    /// D4: `--var NAME=VALUE`, once per name, on `run` and `suite`.
    #[test]
    fn var_takes_a_name_and_value_once_per_name() {
        let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
        };
        assert_eq!(
            run_parts(&["run", "a", "--var", "model=gemma", "--var=url=http://x?a=b"]).0.vars,
            pairs(&[("model", "gemma"), ("url", "http://x?a=b")])
        );
        match command(&["suite", "--var", "model="]) {
            Command::Suite { opts, .. } => assert_eq!(opts.vars, pairs(&[("model", "")])),
            other => panic!("{other:?}"),
        }
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(err(&["run", "a", "--var", "model"]), "--var takes NAME=VALUE, not 'model'");
        assert_eq!(err(&["run", "a", "--var", "m=1", "--var", "m=2"]), "--var m is given twice");
        assert_eq!(
            err(&["run", "a", "--var", "a b=1"]),
            "--var: 'a b' is not a variable name (letters, digits, _ and -)"
        );
        assert_eq!(err(&["show", "--var", "a=1"]), "unknown option '--var'");
    }

    /// D5: the body alone unless `-i` or `-v` asks for more; `-v` holds
    /// everything `-i` shows.
    #[test]
    fn run_shows_the_body_unless_asked_for_more() {
        assert_eq!(run_parts(&["run", "a"]).2, Shown::Body);
        assert_eq!(run_parts(&["run", "a", "-i"]).2, Shown::Include);
        assert_eq!(run_parts(&["run", "a", "--include"]).2, Shown::Include);
        assert_eq!(run_parts(&["run", "a", "-v"]).2, Shown::Verbose);
        assert_eq!(run_parts(&["run", "a", "--verbose", "-i"]).2, Shown::Verbose);
        assert_eq!(parsed(&["suite", "-i"]).unwrap_err(), "unknown option '-i'");
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert!(err(&["run", "a", "-v", "--json"]).contains("--json prints the whole run"));
        assert!(err(&["run", "a", "-i", "--json"]).contains("--json prints the whole run"));
        // A dry run prints the whole report already; -i and -v ask for no
        // more, so neither is refused.
        assert_eq!(run_parts(&["run", "a", "-v", "--dry-run"]).2, Shown::Verbose);
        assert_eq!(run_parts(&["run", "a", "-i", "--dry-run"]).2, Shown::Include);
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
        assert_eq!(err(&["help", "edit", "--json"]), "unexpected argument '--json'");
    }

    #[test]
    fn help_is_the_commands_usage() {
        assert!(help(&["help", "vars"]).starts_with("usage: postui vars"));
        assert!(help(&["vars", "-h"]).starts_with("usage: postui vars"));
        // A command's help needs none of its required arguments.
        assert!(help(&["init", "--help"]).starts_with("usage: postui init"));
        assert_eq!(help(&["help"]), crate::config::USAGE);
    }

    /// Every command but `help` has a usage, and only those do; a group's
    /// commands are its first word's.
    #[test]
    fn every_command_has_a_usage() {
        let mut first: Vec<&str> =
            SPECS.iter().map(|s| s.name.split(' ').next().unwrap()).collect();
        first.dedup();
        assert_eq!(first, SUBCOMMANDS[..SUBCOMMANDS.len() - 1]);
        for spec in &SPECS {
            let line = spec.usage.lines().next().unwrap();
            let named = format!("usage: postui {}", spec.name);
            assert!(line == named || line.starts_with(&format!("{named} ")), "{line}");
        }
        for g in &GROUPS {
            for verb in g.verbs {
                assert!(spec(&format!("{} {verb}", g.name)).is_some(), "{} {verb}", g.name);
            }
        }
    }

    #[test]
    fn edit_takes_its_verb_and_options() {
        assert_eq!(
            command(&["edit", "begin", "--note", "add orders", "-p", "api"]),
            Command::EditBegin {
                g: Global { project: Some("api".into()), json: false },
                note: Some("add orders".into()),
            }
        );
        assert_eq!(parsed(&["edit", "begin", "--force"]).unwrap_err(), "unknown option '--force'");
        assert_eq!(
            command(&["edit", "end", "--json"]),
            Command::EditEnd { g: Global { project: None, json: true }, force: false }
        );
        assert_eq!(command(&["edit", "status"]), Command::EditStatus(Global::default()));
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(err(&["edit"]), "edit needs one of: begin, end, status");
        assert_eq!(err(&["edit", "--json"]), "edit needs one of: begin, end, status");
        assert_eq!(err(&["edit", "start"]), "no command 'edit start'");
        assert_eq!(err(&["edit", "status", "--note", "x"]), "unknown option '--note'");
        assert_eq!(err(&["edit", "end", "--force", "--force"]), "--force is given twice");
    }

    /// `skill install`'s `--project` is a place, not `-p`'s project.
    #[test]
    fn skill_takes_its_verb_and_place() {
        assert_eq!(
            command(&["skill", "install"]),
            Command::SkillInstall { into_project: false, force: false, json: false }
        );
        assert_eq!(
            command(&["skill", "install", "--project", "--force", "--json"]),
            Command::SkillInstall { into_project: true, force: true, json: true }
        );
        assert_eq!(command(&["skill", "install", "--user"]), command(&["skill", "install"]));
        assert_eq!(command(&["skill", "show"]), Command::SkillShow);
        let err = |args: &[&str]| parsed(args).unwrap_err();
        assert_eq!(
            err(&["skill", "install", "--user", "--project"]),
            "--user and --project name two places; give one"
        );
        assert_eq!(err(&["skill", "install", "--project", "x"]), "unexpected argument 'x'");
        assert_eq!(err(&["skill", "install", "-p"]), "unknown option '-p'");
        assert_eq!(err(&["skill", "show", "--json"]), "unknown option '--json'");
        assert_eq!(err(&["skill"]), "skill needs one of: install, show");
        assert!(help(&["help", "skill", "install"]).starts_with("usage: postui skill install "));
    }

    #[test]
    fn edit_help_is_the_groups_or_the_verbs() {
        assert!(help(&["edit", "--help"]).starts_with("usage: postui edit begin|end|status"));
        assert!(help(&["help", "edit"]).starts_with("usage: postui edit begin|end|status"));
        assert!(help(&["help", "edit", "end"]).starts_with("usage: postui edit end "));
        assert!(help(&["edit", "begin", "-h"]).starts_with("usage: postui edit begin "));
        assert_eq!(parsed(&["help", "edit", "start"]).unwrap_err(), "no command 'edit start'");
        assert_eq!(parsed(&["help", "edit", "end", "x"]).unwrap_err(), "unexpected argument 'x'");
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(usage_of(&args(&["edit", "end", "--bogus"])), EDIT_END_USAGE);
        assert_eq!(usage_of(&args(&["edit", "start"])), EDIT_USAGE);
        assert_eq!(usage_of(&args(&["show", "x"])), SHOW_USAGE);
    }
}
