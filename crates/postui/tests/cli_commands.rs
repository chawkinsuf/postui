//! The commands (`postui projects`, `postui show`, ...), run for real in a
//! throwaway home: what they print, the exit status a script sees, and
//! that a read writes nothing (spec 2026-10-28 §4, R16).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A throwaway home: `HOME` and the XDG directories point into it, so the
/// binary reads (and could only ever write) its config here. The path is
/// canonical, so a root the binary builds from its working directory
/// (macOS's temp dirs sit behind a symlink) compares equal to ours.
struct Home {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap();
        Home { _dir: dir, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// postui's config directory in this home (`$XDG_CONFIG_HOME/postui`).
    fn config_dir(&self) -> PathBuf {
        self.path.join("config").join("postui")
    }

    /// Writes `config.toml` with `roots` as the registered projects.
    fn register(&self, roots: &[&Path]) {
        let known: Vec<String> =
            roots.iter().map(|r| format!("{:?}", r.to_str().unwrap())).collect();
        std::fs::create_dir_all(self.config_dir()).unwrap();
        std::fs::write(
            self.config_dir().join("config.toml"),
            format!("[projects]\nknown = [{}]\n", known.join(", ")),
        )
        .unwrap();
    }

    /// A new project at `<home>/<dir>`, named `name`.
    fn project(&self, dir: &str, name: &str) -> PathBuf {
        let root = self.path.join(dir);
        postui_core::fixtures::init_project(&root, Some(name)).unwrap();
        root
    }

    /// Runs the binary from the home directory.
    fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Output {
        self.run_in(&self.path, args, |_| {})
    }

    /// Runs the binary from `cwd`, with stdin closed and the command
    /// adjusted first (a stream sent elsewhere).
    fn run_in<S: AsRef<OsStr>>(
        &self,
        cwd: &Path,
        args: &[S],
        adjust: impl FnOnce(&mut Command),
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_postui"));
        command
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.path)
            .env("XDG_CONFIG_HOME", self.path.join("config"))
            .env("XDG_DATA_HOME", self.path.join("data"))
            .env("XDG_STATE_HOME", self.path.join("state"))
            .stdin(Stdio::null());
        adjust(&mut command);
        command.output().expect("the binary runs")
    }
}

/// `p` as the `&str` an argument list and a JSON value hold.
fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The command's stdout, parsed as the one JSON document it must be.
fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Asserts a run succeeded, showing what it said when it didn't.
fn assert_ok(out: &Output, what: &str) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{what}: stdout {} stderr {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every file and directory under `root` with its bytes (a directory's are
/// empty): two calls compare equal only if nothing was written in between.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(rel, Vec::new());
                walk(&path, root, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn not_a_project_is_an_error_envelope() {
    let home = Home::new();
    let empty = home.path().join("empty");
    std::fs::create_dir(&empty).unwrap();

    let out = home.run(&["show", "-p", s(&empty)]);
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("not a postui project"), "{error}");
    assert_eq!(v["file"], s(&empty.join("project.toml")));

    let out = home.run(&["show", "-p", s(&empty), "--text"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("postui: not a postui project"), "{stderr}");
}

/// Review Focus 3: a project's subdirectory is not the project; the error
/// says how to name it.
#[test]
fn subdirectory_of_a_project_is_not_a_project() {
    let home = Home::new();
    let proj = home.project("api", "API");
    let sub = proj.join("requests").join("main");
    std::fs::create_dir_all(&sub).unwrap();

    let out = home.run_in(&sub, &["show"], |_| {});
    assert_eq!(out.status.code(), Some(2));
    let v = json(&out);
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("not a postui project") && error.contains("-p"), "{error}");
    assert_eq!(v["file"], s(&sub.join("project.toml")));
}

/// Review Focus 2: every spelling of one directory is one root, `..`
/// included (the natural answer to the "pass -p" hint from inside it).
#[test]
fn project_path_forms_resolve_alike() {
    let home = Home::new();
    let proj = home.project("api", "API");
    let main = proj.join("requests").join("main");
    std::fs::create_dir_all(&main).unwrap();
    let trailing = format!("{}/", s(&proj));
    let up = format!("{}/requests/..", s(&proj));
    for form in [s(&proj), &trailing, &up, "api", "./api", "api/", "~/api", "api/requests/.."] {
        let out = home.run(&["show", "-p", form]);
        assert_ok(&out, form);
        assert_eq!(json(&out)["root"], s(&proj), "{form}");
    }
    for (cwd, args) in [(&main, &["show", "-p", "../.."][..]), (&proj, &["show"])] {
        let out = home.run_in(cwd, args, |_| {});
        assert_ok(&out, &format!("{args:?} from {}", cwd.display()));
        assert_eq!(json(&out)["root"], s(&proj), "{args:?} from {}", cwd.display());
    }
}

#[test]
fn projects_lists_the_registry() {
    let home = Home::new();
    let proj = home.project("billing", "Billing API");
    let gone = home.path().join("gone");
    home.register(&[&proj, &gone]);
    let before = tree(&home.config_dir());

    let out = home.run(&["projects"]);
    assert_ok(&out, "projects");
    let v = json(&out);
    assert_eq!(
        v["projects"][0],
        serde_json::json!({"name": "Billing API", "root": s(&proj), "exists": true})
    );
    assert_eq!(v["projects"][1]["root"], s(&gone));
    assert_eq!(v["projects"][1]["exists"], false);
    assert_eq!(v["projects"].as_array().unwrap().len(), 2);
    let config_dir = v["config_dir"].as_str().unwrap();
    assert!(Path::new(config_dir).ends_with("config/postui"), "{config_dir}");

    let out = home.run(&["projects", "--text"]);
    assert_ok(&out, "projects --text");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Billing API") && text.contains(s(&gone)), "{text}");

    assert_eq!(tree(&home.config_dir()), before, "the config dir is untouched");
}

#[test]
fn registered_name_resolves() {
    let home = Home::new();
    let proj = home.project("billing", "Billing API");
    home.register(&[&proj]);

    for name in ["Billing API", "billing api"] {
        let out = home.run(&["show", "-p", name]);
        assert_ok(&out, name);
        assert_eq!(json(&out)["root"], s(&proj), "{name}");
    }

    let out = home.run(&["show", "-p", "Payments"]);
    assert_eq!(out.status.code(), Some(2));
    let error = json(&out)["error"].as_str().unwrap().to_string();
    assert!(error.contains("no such directory or registered project \"Payments\""), "{error}");

    let twin = home.project("other", "Billing API");
    home.register(&[&proj, &twin]);
    let out = home.run(&["show", "-p", "Billing API"]);
    assert_eq!(out.status.code(), Some(2));
    let error = json(&out)["error"].as_str().unwrap().to_string();
    assert!(error.contains("names 2 registered projects"), "{error}");
    assert!(error.contains(s(&proj)) && error.contains(s(&twin)), "{error}");
}

/// Review Focus 5: a reader that stopped ends the run quietly.
#[test]
fn closed_stdout_ends_quietly() {
    let home = Home::new();
    let proj = home.project("api", "API");
    for args in [&["projects"][..], &["show", "-p", s(&proj)]] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = home.run_in(home.path(), args, |c| {
            c.stdout(writer);
        });
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// A command's help is its own usage on stdout; a mistake is the error
/// envelope on stdout, or with `--text` the reason and the command's own
/// usage on stderr; status 2 either way (spec 2026-10-28 §5).
#[test]
fn command_help_and_mistakes() {
    let home = Home::new();
    for (args, usage) in [
        (&["help", "vars"][..], "usage: postui vars"),
        (&["vars", "-h"], "usage: postui vars"),
        (&["show", "--help"], "usage: postui show"),
        (&["help"], "usage: postui"),
    ] {
        let out = home.run(args);
        assert_ok(&out, &format!("{args:?}"));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with(usage), "{args:?}: {stdout}");
    }
    let out = home.run(&["show", "--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out), serde_json::json!({"error": "unknown option '--bogus'"}));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));

    let out = home.run(&["show", "--bogus", "--text"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("postui: unknown option '--bogus'\n\n"), "{stderr}");
    assert!(stderr.contains("usage: postui show "), "the command's own usage: {stderr}");
    assert!(!stderr.contains("commands ("), "not the general usage: {stderr}");
}
