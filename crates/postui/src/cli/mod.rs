//! `postui <command>`: the project's contents on the command line, for
//! scripts and agents (spec 2026-10-28 §4). The first argument picks the
//! command (`config::parse_cli` routes it here); [`parse`] reads the rest,
//! [`run`] runs it and prints one JSON report, or `--text` for people.
//! Read commands open the project read-only, so they write nothing (R16).

pub mod out;
mod read;
pub mod target;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// The options every project command takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Global {
    /// `-p`: a directory, or a registered project's name; `None` is the
    /// current directory.
    pub project: Option<String>,
    /// `--text`: print for people instead of JSON.
    pub text: bool,
}

/// A parsed command line, one variant per command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Projects { text: bool },
    Show(Global),
    Requests { g: Global, space: Option<String> },
    Vars { g: Global, env: Option<String>, show_secrets: bool },
    Check { g: Global, env: Option<String> },
    Init { dir: PathBuf, name: Option<String>, text: bool },
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
pub const SUBCOMMANDS: &[&str] = &["projects", "show", "requests", "vars", "check", "init", "help"];

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

/// The `--text` line every command's usage carries.
macro_rules! text_flag {
    () => {
        "  --text                    print for people instead of JSON
"
    };
}

const PROJECTS_USAGE: &str = concat!(
    "\
usage: postui projects [--text]

The projects postui knows: each one's name, root, and whether the root is
still there; and the config directory they are listed in.

",
    text_flag!()
);

const SHOW_USAGE: &str = concat!(
    "\
usage: postui show [-p <dir|name>] [--text]

The project's name, root and files, its environments and spaces, and the
warnings opening it gave.

",
    project_flag!(),
    text_flag!()
);

const REQUESTS_USAGE: &str = concat!(
    "\
usage: postui requests [-p <dir|name>] [--space <space>] [--text]

The project's requests in sidebar order: each one's slug, name, method,
URL as written, and file.

",
    project_flag!(),
    "  --space <space>           only this space's requests
",
    text_flag!()
);

const VARS_USAGE: &str = concat!(
    "\
usage: postui vars [-p <dir|name>] [--env <env>] [--show-secrets] [--text]

Every variable's value in an environment and where it comes from, and
every selector with its options and the one selected. Secrets are masked.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
  --show-secrets            print secrets' values
",
    text_flag!()
);

const CHECK_USAGE: &str = concat!(
    "\
usage: postui check [-p <dir|name>] [--env <env>] [--text]

Parses and validates every project file, then resolves every request's
variables against an environment. Exits 1 when it finds an error.

",
    project_flag!(),
    "  --env <env>               the environment, by slug or name (default: the
                            one postui has open, else the first)
",
    text_flag!()
);

const INIT_USAGE: &str = concat!(
    "\
usage: postui init <dir> [--name <name>] [--text]

Starts a project in <dir> with the fewest files one needs. Never
overwrites a file.

  --name <name>             the project's name (default: the directory's)
",
    text_flag!()
);

/// One command: its name, the options it takes (by their long spelling;
/// `-p` is `--project`), whether it takes a directory, and its usage.
struct Spec {
    name: &'static str,
    flags: &'static [&'static str],
    takes_dir: bool,
    usage: &'static str,
}

#[rustfmt::skip]
const SPECS: [Spec; 6] = [
    Spec { name: "projects", flags: &["--text"],                                          takes_dir: false, usage: PROJECTS_USAGE },
    Spec { name: "show",     flags: &["--project", "--text"],                             takes_dir: false, usage: SHOW_USAGE },
    Spec { name: "requests", flags: &["--project", "--text", "--space"],                  takes_dir: false, usage: REQUESTS_USAGE },
    Spec { name: "vars",     flags: &["--project", "--text", "--env", "--show-secrets"], takes_dir: false, usage: VARS_USAGE },
    Spec { name: "check",    flags: &["--project", "--text", "--env"],                    takes_dir: false, usage: CHECK_USAGE },
    Spec { name: "init",     flags: &["--text", "--name"],                                takes_dir: true,  usage: INIT_USAGE },
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
    let g = Global { project: options.project, text: options.text };
    Ok(Parsed::Run(match spec.name {
        "projects" => Command::Projects { text: g.text },
        "show" => Command::Show(g),
        "requests" => Command::Requests { g, space: options.space },
        "vars" => Command::Vars { g, env: options.env, show_secrets: options.show_secrets },
        "check" => Command::Check { g, env: options.env },
        _ => Command::Init {
            dir: options.dir.ok_or("init needs a directory")?,
            name: options.name,
            text: g.text,
        },
    }))
}

/// Every option a command line can carry, before the command picks its own.
#[derive(Default)]
struct Options {
    project: Option<String>,
    text: bool,
    space: Option<String>,
    env: Option<String>,
    show_secrets: bool,
    name: Option<String>,
    dir: Option<PathBuf>,
}

/// Reads `args` (help flags already out) against what `spec` takes.
/// A value comes as the next argument or after `=` (`--space=main`).
fn scan(spec: &Spec, args: &[&OsString]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if !text.starts_with('-') {
            if !spec.takes_dir {
                return Err(format!("unexpected argument '{text}'"));
            }
            if o.dir.is_some() {
                return Err(format!("one directory at most; '{text}' is a second"));
            }
            o.dir = Some(match arg.to_str() {
                Some(s) => crate::config::expand_tilde(s),
                None => PathBuf::from(arg),
            });
            continue;
        }
        let (spelled, inline) = match text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (&*text, None),
        };
        let flag = if spelled == "-p" { "--project" } else { spelled };
        if !spec.flags.contains(&flag) {
            return Err(format!("unknown option '{text}'"));
        }
        let switch = match flag {
            "--text" => Some(&mut o.text),
            "--show-secrets" => Some(&mut o.show_secrets),
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
            "--project" => &mut o.project,
            "--space" => &mut o.space,
            "--env" => &mut o.env,
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
/// status to exit with: 0 on success, 2 on an error.
pub async fn run(cmd: Command) -> ExitCode {
    match cmd {
        Command::Projects { text } => finish(read::projects(), text, read::ProjectsReport::text),
        Command::Show(g) => finish(read::show(&g), g.text, read::ShowReport::text),
        Command::Requests { g, space } => {
            finish(read::requests(&g, space.as_deref()), g.text, read::RequestsReport::text)
        }
        Command::Vars { g, env, show_secrets } => {
            finish(read::vars(&g, env.as_deref(), show_secrets), g.text, read::VarsReport::text)
        }
        Command::Check { g, .. } => not_yet("check", g.text),
        Command::Init { text, .. } => not_yet("init", text),
    }
}

/// Prints a command's report, or its error.
fn finish<T: serde::Serialize>(
    result: Result<T, CliError>,
    text: bool,
    render_text: impl FnOnce(&T) -> String,
) -> ExitCode {
    match result {
        Ok(report) => out::emit(&report, text, render_text).unwrap_or(ExitCode::SUCCESS),
        Err(e) => out::fail(&e, text),
    }
}

/// A command whose report is still to come (#214's later steps).
fn not_yet(name: &str, text: bool) -> ExitCode {
    out::fail(&CliError::new(format!("postui {name} is not implemented yet")), text)
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
    fn show_takes_the_project_and_text() {
        assert_eq!(
            command(&["show", "-p", "x", "--text"]),
            Command::Show(Global { project: Some("x".into()), text: true })
        );
        assert_eq!(
            command(&["show", "--project=x"]),
            Command::Show(Global { project: Some("x".into()), text: false })
        );
    }

    #[test]
    fn requests_takes_a_space() {
        assert_eq!(
            command(&["requests", "--space", "main"]),
            Command::Requests {
                g: Global { project: None, text: false },
                space: Some("main".into())
            }
        );
    }

    #[test]
    fn vars_check_and_init_take_their_options() {
        assert_eq!(
            command(&["vars", "--env", "dev", "--show-secrets"]),
            Command::Vars { g: Global::default(), env: Some("dev".into()), show_secrets: true }
        );
        assert_eq!(
            command(&["check", "--env=dev", "-p", "api"]),
            Command::Check {
                g: Global { project: Some("api".into()), text: false },
                env: Some("dev".into())
            }
        );
        assert_eq!(
            command(&["init", "kit", "--name", "Kit"]),
            Command::Init { dir: PathBuf::from("kit"), name: Some("Kit".into()), text: false }
        );
        assert_eq!(command(&["projects"]), Command::Projects { text: false });
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
        assert_eq!(err(&["show", "--text", "--text"]), "--text is given twice");
        assert_eq!(err(&["show", "--text=yes"]), "--text takes no value");
        assert_eq!(err(&["show", "api"]), "unexpected argument 'api'");
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
