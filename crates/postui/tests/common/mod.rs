//! What the command tests share: a throwaway home to run the binary in, a
//! fixture project, and helpers to read what a run printed and to prove it
//! wrote nothing.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A throwaway home: `HOME` and the XDG directories point into it, so the
/// binary reads (and could only ever write) its config here. The path is
/// canonical, so a root the binary builds from its working directory
/// (macOS's temp dirs sit behind a symlink) compares equal to ours.
pub struct Home {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap();
        Home { _dir: dir, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// postui's config directory in this home (`$XDG_CONFIG_HOME/postui`).
    pub fn config_dir(&self) -> PathBuf {
        self.path.join("config").join("postui")
    }

    /// Writes `config.toml` with `roots` as the registered projects.
    pub fn register(&self, roots: &[&Path]) {
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
    pub fn project(&self, dir: &str, name: &str) -> PathBuf {
        let root = self.path.join(dir);
        postui_core::fixtures::init_project(&root, Some(name)).unwrap();
        root
    }

    /// Runs the binary from the home directory.
    pub fn run<S: AsRef<OsStr>>(&self, args: &[S]) -> Output {
        self.run_in(&self.path, args, |_| {})
    }

    /// Runs the binary from `cwd`, with stdin closed and the command
    /// adjusted first (a stream sent elsewhere).
    pub fn run_in<S: AsRef<OsStr>>(
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
pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The command's stdout, parsed as the one JSON document it must be.
pub fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Asserts a run succeeded, showing what it said when it didn't.
pub fn assert_ok(out: &Output, what: &str) {
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
pub fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
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

/// A project at `<home>/<dir>` with two spaces, two environments and three
/// requests, its name full of characters JSON must escape.
pub fn write_project(home: &Home, dir: &str) -> PathBuf {
    let root = home.path().join(dir);
    let put = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    put(
        "project.toml",
        "name = \"Billing \\\"API\\\" {{x}} ✓\"\nspaces = [\"main\", \"admin\"]\n\n\
         [space.admin]\nname = \"Admin\"\n\n[environment.dev]\nname = \"Dev\"\n",
    );
    put(
        "variables.toml",
        "[base]\ndescription = \"API root\"\n\n[token]\nsecret = true\n\n[id]\n\n\
         [region]\ndefault = \"eu\"\n\n[selectors.user]\nfields = [\"user_id\"]\n",
    );
    put(
        "environments/dev.toml",
        "base = \"https://dev\"\n\n[options.user.alice]\nuser_id = \"42\"\n",
    );
    put("environments/qa.toml", "base = \"https://qa\"\n\n[options.user.bob]\nuser_id = \"7\"\n");
    put(".postui/state.toml", "environment = \"dev\"\n\n[selections.dev]\nuser = \"alice\"\n");
    put(".postui/secrets.toml", "[dev]\ntoken = \"s3cr3t-value\"\n");
    put(
        "requests/main/users/get-user.toml",
        "name = \"Get user\"\nmethod = \"GET\"\nurl = \"{{base}}/users\"\n",
    );
    put("requests/main/list.toml", "name = \"List\"\nmethod = \"POST\"\nurl = \"{{base}}/list\"\n");
    put("requests/admin/x.toml", "name = \"X\"\nmethod = \"DELETE\"\nurl = \"{{base}}/x\"\n");
    root
}
