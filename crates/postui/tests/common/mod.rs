//! What the command tests share: a throwaway home to run the binary in, a
//! fixture project, and helpers to read what a run printed and to prove it
//! wrote nothing.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

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
        let mut command = self.command(cwd, args);
        adjust(&mut command);
        command.output().expect("the binary runs")
    }

    /// The binary as [`Home::run_in`] runs it, not yet started, for a test
    /// that handles the process itself.
    pub fn command<S: AsRef<OsStr>>(&self, cwd: &Path, args: &[S]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_postui"));
        command
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.path)
            .env("XDG_CONFIG_HOME", self.path.join("config"))
            .env("XDG_DATA_HOME", self.path.join("data"))
            .env("XDG_STATE_HOME", self.path.join("state"))
            .stdin(Stdio::null());
        command
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

/// A local HTTP server for the commands that send. It answers by path:
/// `/ok` 200 JSON `{"id":42}`; `/missing` 404 `nope`; `/echo` 200 with the
/// request's `authorization` in `x-echo` and its body back; `/bytes` 200
/// `ok\xff\xfe`; `/escaped` 200 JSON holding the
/// fixture's secret with its hyphen written as a JSON escape;
/// `/echo-url` 200 with the request target as the body; `/escaped-slash`
/// 200 JSON holding `ab/cd/ef-gh` written `ab\/cd\/ef-gh`; `/echo-go` 200
/// JSON `{"auth": <the request's authorization>}` as Go escapes it; `/hang`
/// never answers; anything else 200, empty.
pub struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        // Sockets of `/hang` requests, kept open so they stay unanswered.
        let held = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, held) = (log.clone(), held.clone());
                std::thread::spawn(move || serve(stream, &log, &held));
            }
        });
        Server { port, seen }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// `"METHOD /path"` for each request, in arrival order.
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// Reads one request off `stream`, records it and answers it.
fn serve(mut stream: TcpStream, seen: &Mutex<Vec<String>>, held: &Mutex<Vec<TcpStream>>) {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => data.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default();
    let target = request_line.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default();
    let header = |name: &str| {
        lines
            .clone()
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
    };
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while data.len() < head_end + length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&chunk[..n]),
        }
    }
    let body = &data[head_end..];
    seen.lock().unwrap().push(format!("{method} {path}"));
    let (status, extra, payload): (&str, String, Vec<u8>) = match path {
        "/ok" => ("200 OK", "content-type: application/json\r\n".into(), br#"{"id":42}"#.to_vec()),
        "/missing" => ("404 Not Found", String::new(), b"nope".to_vec()),
        "/echo" => (
            "200 OK",
            format!("x-echo: {}\r\n", header("authorization").unwrap_or_default()),
            body.to_vec(),
        ),
        // The fixture's secret `s3cr3t-value`, its `-` written as an escape.
        "/escaped" => (
            "200 OK",
            "content-type: application/json\r\n".into(),
            // A backslash, `u`, `002d`: six characters on the wire.
            b"{\"t\":\"s3cr3t\\u002dvalue\"}".to_vec(),
        ),
        // The request target, query string included, back as the body.
        "/echo-url" => ("200 OK", String::new(), target.as_bytes().to_vec()),
        // The request's `authorization` in JSON as Go's encoder writes it.
        "/echo-go" => (
            "200 OK",
            "content-type: application/json\r\n".into(),
            format!("{{\"auth\":\"{}\"}}", go_json(&header("authorization").unwrap_or_default()))
                .into_bytes(),
        ),
        // `ab/cd/ef-gh` with its slashes written `\/`, as PHP's json_encode does.
        "/escaped-slash" => (
            "200 OK",
            "content-type: application/json\r\n".into(),
            b"{\"t\":\"ab\\/cd\\/ef-gh\"}".to_vec(),
        ),
        "/bytes" => ("200 OK", String::new(), b"ok\xff\xfe".to_vec()),
        "/hang" => {
            held.lock().unwrap().push(stream);
            return;
        }
        _ => ("200 OK", String::new(), Vec::new()),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&payload);
}

/// `text` as the inside of a JSON string the way Go's `encoding/json`
/// writes it: `"` and `\` escaped, and `&`, `<`, `>` as `&`, `<`,
/// `>`.
fn go_json(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            '&' | '<' | '>' => format!("\\u{:04x}", c as u32),
            other => other.to_string(),
        })
        .collect()
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
