//! A pasted curl command as postui requests (spec §4).

use super::shell::{self, ShellCommand, ShellError};
use crate::model::{Body, Entry, HttpRequest, Method};
use base64::Engine as _;
use indexmap::IndexMap;
use std::fmt;

/// One curl command of a paste, as a request ready to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlImport {
    pub request: HttpRequest,
    /// What did not make it into the request, in this order: dropped
    /// flags as written with their value (`-F file=@a.png`; `-g`/`--globoff`
    /// is lossless and not listed), the earlier
    /// URLs a later one replaced, `$(…) kept as text` notes, then a
    /// pipeline tail or redirect (`| jq .`).
    pub ignored: Vec<String>,
    /// Shell variables converted to `{{NAME}}`, first-seen order.
    pub converted: Vec<String>,
    /// `{METHOD} {last non-empty path segment, else host}`. Never holds a
    /// `/` (a `/` in a display path means a folder).
    pub suggested_name: String,
}

/// Every curl command of a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlBatch {
    /// One per curl command, in paste order; never empty.
    pub commands: Vec<CurlImport>,
    /// Commands that were not curl (`export TOKEN=…`, `echo`). Blank lines
    /// and comments are not commands and are not counted.
    pub skipped_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurlErrorKind {
    NotCurl,
    UnclosedQuote,
    NoUrl,
    CmdStyle,
    /// The flag as written (`-H`, `--header`).
    MissingValue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlError {
    /// The failing command's position in the paste (1-based), when the
    /// paste holds more than one command.
    pub command: Option<usize>,
    pub kind: CurlErrorKind,
}

impl fmt::Display for CurlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(n) = self.command {
            write!(f, "command {n}: ")?;
        }
        match &self.kind {
            CurlErrorKind::NotCurl => f.write_str("clipboard doesn't hold a curl command"),
            CurlErrorKind::UnclosedQuote => f.write_str("unclosed quote"),
            CurlErrorKind::NoUrl => f.write_str("no URL in the curl command"),
            CurlErrorKind::CmdStyle => {
                f.write_str("this is a Windows cmd command \u{2014} copy as cURL (bash) instead")
            }
            CurlErrorKind::MissingValue(flag) => write!(f, "{flag} needs a value"),
        }
    }
}

impl std::error::Error for CurlError {}

/// Every curl command in `text`. Any command that fails fails the whole
/// paste (spec R6): nothing is half-imported.
pub fn parse(text: &str) -> Result<CurlBatch, CurlError> {
    let commands = shell::split(text).map_err(|e| match e {
        ShellError::CmdStyle => CurlError {
            command: None,
            kind: CurlErrorKind::CmdStyle,
        },
        // A broken quote swallows whatever follows, so the commands
        // before it plus itself are all that is known to exist.
        ShellError::Unclosed { command } => CurlError {
            command: (command > 1).then_some(command),
            kind: CurlErrorKind::UnclosedQuote,
        },
    })?;
    let many = commands.len() > 1;
    let mut batch = CurlBatch {
        commands: Vec::new(),
        skipped_lines: 0,
    };
    for (i, cmd) in commands.into_iter().enumerate() {
        if !is_curl(&cmd.words[0]) {
            batch.skipped_lines += 1;
            continue;
        }
        let import = parse_command(cmd).map_err(|kind| CurlError {
            command: many.then_some(i + 1),
            kind,
        })?;
        batch.commands.push(import);
    }
    if batch.commands.is_empty() {
        return Err(CurlError {
            command: None,
            kind: CurlErrorKind::NotCurl,
        });
    }
    Ok(batch)
}

fn is_curl(word: &str) -> bool {
    let base = word.rsplit('/').next().unwrap_or(word);
    base == "curl" || base == "curl.exe"
}

/// curl's long options that take a value — enough of curl's own list
/// that an unknown flag's value is never mistaken for the URL. A flag
/// not here is taken as valueless.
const VALUE_FLAGS: &[&str] = &[
    "abstract-unix-socket", "alt-svc", "aws-sigv4", "cacert", "capath", "cert", "cert-type",
    "ciphers", "config", "connect-timeout", "connect-to", "continue-at", "cookie", "cookie-jar",
    "create-file-mode", "crlfile", "curves", "data", "data-ascii", "data-binary", "data-raw",
    "data-urlencode", "delegation", "dns-interface", "dns-ipv4-addr", "dns-ipv6-addr",
    "dns-servers", "doh-url", "dump-header", "ech", "egd-file", "engine", "etag-compare",
    "etag-save", "expect100-timeout", "form", "form-string", "ftp-account",
    "ftp-alternative-to-user", "ftp-method", "ftp-port", "ftp-ssl-ccc-mode",
    "happy-eyeballs-timeout-ms", "haproxy-clientip", "header", "hostpubmd5", "hostpubsha256",
    "hsts", "interface", "ip-tos", "ipfs-gateway", "json", "keepalive-cnt", "keepalive-time",
    "key", "key-type", "krb", "libcurl", "limit-rate", "local-port", "login-options",
    "mail-auth", "mail-from", "mail-rcpt", "max-filesize", "max-redirs", "max-time",
    "netrc-file", "noproxy", "oauth2-bearer", "output", "output-dir", "parallel-max", "pass",
    "pinnedpubkey", "preproxy", "proto", "proto-default", "proto-redir", "proxy",
    "proxy-cacert", "proxy-capath", "proxy-cert", "proxy-cert-type", "proxy-ciphers",
    "proxy-crlfile", "proxy-header", "proxy-key", "proxy-key-type", "proxy-pass",
    "proxy-pinnedpubkey", "proxy-service-name", "proxy-tls13-ciphers", "proxy-tlsauthtype",
    "proxy-tlspassword", "proxy-tlsuser", "proxy-user", "proxy1.0", "pubkey", "quote",
    "random-file", "range", "rate", "referer", "request", "request-target", "resolve",
    "retry", "retry-delay", "retry-max-time", "sasl-authzid", "service-name", "socks4",
    "socks4a", "socks5", "socks5-gssapi-service", "socks5-hostname", "speed-limit",
    "speed-time", "stderr", "telnet-option", "tftp-blksize", "time-cond", "tls-max",
    "tls13-ciphers", "tlsauthtype", "tlspassword", "tlsuser", "trace", "trace-ascii",
    "trace-config", "unix-socket", "upload-file", "url", "url-query", "user", "user-agent",
    "variable", "write-out",
];

fn takes_value(long: &str) -> bool {
    VALUE_FLAGS.contains(&long)
}

/// A short flag's long name (curl's man page). `None` for a letter curl
/// does not define, which is then taken as valueless.
fn short_to_long(c: char) -> Option<&'static str> {
    Some(match c {
        'X' => "request",
        'H' => "header",
        'd' => "data",
        'u' => "user",
        'b' => "cookie",
        'A' => "user-agent",
        'e' => "referer",
        'G' => "get",
        'I' => "head",
        'k' => "insecure",
        'F' => "form",
        'o' => "output",
        'T' => "upload-file",
        'x' => "proxy",
        'm' => "max-time",
        'w' => "write-out",
        'c' => "cookie-jar",
        'E' => "cert",
        'K' => "config",
        'r' => "range",
        'U' => "proxy-user",
        'y' => "speed-time",
        'Y' => "speed-limit",
        'z' => "time-cond",
        'C' => "continue-at",
        'D' => "dump-header",
        'P' => "ftp-port",
        'Q' => "quote",
        't' => "telnet-option",
        's' => "silent",
        'S' => "show-error",
        'L' => "location",
        'v' => "verbose",
        'i' => "include",
        'f' => "fail",
        'N' => "no-buffer",
        'O' => "remote-name",
        'J' => "remote-header-name",
        'g' => "globoff",
        'n' => "netrc",
        'j' => "junk-session-cookies",
        'Z' => "parallel",
        'R' => "remote-time",
        'l' => "list-only",
        'M' => "manual",
        'p' => "proxytunnel",
        'q' => "disable",
        'B' => "use-ascii",
        'a' => "append",
        'V' => "version",
        'h' => "help",
        '#' => "progress-bar",
        '0' => "http1.0",
        '1' => "tlsv1",
        '2' => "sslv2",
        '3' => "sslv3",
        '4' => "ipv4",
        '6' => "ipv6",
        _ => return None,
    })
}

/// What one command's flags said, before it becomes a request.
#[derive(Default)]
struct Flags {
    urls: Vec<String>,
    /// The flag as written, and its value.
    method: Option<(String, String)>,
    head: bool,
    get: bool,
    headers: Vec<(String, String)>,
    data: Vec<String>,
    json: bool,
    /// The flag as written, and its value.
    user: Option<(String, String)>,
    cookies: Vec<String>,
    user_agent: Option<String>,
    referer: Option<String>,
    insecure: bool,
    /// The method curl would send a dropped body with (a file or form
    /// flag): POST, or PUT for `-T` (spec D10).
    dropped_body: Option<Method>,
    ignored: Vec<String>,
}

impl Flags {
    /// One flag: `display` is how it was written (`-H`, `--header`),
    /// `long` its long name (empty for an unknown short letter).
    fn apply(&mut self, display: &str, long: &str, value: Option<String>) {
        match (long, value) {
            ("url", Some(v)) => self.urls.push(v),
            ("request", Some(v)) => self.method = Some((display.to_string(), v)),
            ("head", None) => self.head = true,
            ("get", None) => self.get = true,
            ("insecure", None) => self.insecure = true,
            // Lossless: postui never globs a URL (D9).
            ("globoff", None) => {}
            ("header", Some(v)) => self.header(display, v),
            ("data" | "data-ascii" | "data-binary" | "json", Some(v)) if v.starts_with('@') => {
                self.drop_body(display, v, Method::Post)
            }
            ("form" | "form-string", Some(v)) => self.drop_body(display, v, Method::Post),
            ("upload-file", Some(v)) => self.drop_body(display, v, Method::Put),
            ("data" | "data-ascii" | "data-binary" | "data-raw", Some(v)) => self.data.push(v),
            ("json", Some(v)) => {
                self.data.push(v);
                self.json = true;
            }
            ("data-urlencode", Some(v)) => match urlencode_data(&v) {
                Some(d) => self.data.push(d),
                None => self.drop_body(display, v, Method::Post),
            },
            ("user", Some(v)) => self.user = Some((display.to_string(), v)),
            ("cookie", Some(v)) if v.contains('=') => self.cookies.push(v),
            ("user-agent", Some(v)) => self.user_agent = Some(v),
            ("referer", Some(v)) => self.referer = Some(v),
            (_, value) => self.ignore(display, value),
        }
    }

    fn ignore(&mut self, display: &str, value: Option<String>) {
        self.ignored.push(match value {
            Some(v) => format!("{display} {v}"),
            None => display.to_string(),
        });
    }

    /// A body flag whose content cannot be imported (a file, a form): the
    /// flag is listed as ignored, but curl would still have sent `method`.
    fn drop_body(&mut self, display: &str, value: String, method: Method) {
        self.ignore(display, Some(value));
        self.dropped_body.get_or_insert(method);
    }

    /// `Name: value`; `Name;` is an empty value; `Name:` with nothing
    /// after it removes a header in curl, so it is dropped.
    fn header(&mut self, display: &str, raw: String) {
        if let Some((name, value)) = raw.split_once(':') {
            let value = value.trim_start();
            if value.is_empty() {
                return self.ignore(display, Some(raw));
            }
            self.headers.push((name.trim().to_string(), value.to_string()));
        } else if let Some(name) = raw.strip_suffix(';') {
            self.headers.push((name.trim().to_string(), String::new()));
        } else {
            self.ignore(display, Some(raw));
        }
    }
}

/// `--data-urlencode`'s forms (curl's man page): `content`, `=content`
/// and `name=content` encode; `@file` and `name@file` read a file, which
/// an import cannot do (`None`).
fn urlencode_data(v: &str) -> Option<String> {
    match (v.find('='), v.find('@')) {
        (Some(e), at) if at.is_none_or(|a| e < a) => {
            let (name, content) = (&v[..e], &v[e + 1..]);
            Some(if name.is_empty() {
                percent_encode(content)
            } else {
                format!("{name}={}", percent_encode(content))
            })
        }
        (_, Some(_)) => None,
        _ => Some(percent_encode(v)),
    }
}

/// curl's `curl_easy_escape`: unreserved bytes stay, the rest are `%XX`.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn parse_command(cmd: ShellCommand) -> Result<CurlImport, CurlErrorKind> {
    let ShellCommand {
        words,
        tail,
        converted,
        substitutions,
    } = cmd;
    let mut flags = Flags::default();
    let mut words = words.into_iter().skip(1);
    let mut options_done = false;
    while let Some(word) = words.next() {
        if options_done || !word.starts_with('-') || word == "-" {
            flags.urls.push(word);
        } else if word == "--" {
            options_done = true;
        } else if let Some(long) = word.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (long.to_string(), None),
            };
            let display = format!("--{name}");
            let value = match inline {
                Some(v) => Some(v),
                None if takes_value(&name) => Some(
                    words
                        .next()
                        .ok_or_else(|| CurlErrorKind::MissingValue(display.clone()))?,
                ),
                None => None,
            };
            flags.apply(&display, &name, value);
        } else {
            // Short flags, possibly bundled (`-sSL`); a value-taking one
            // takes the rest of the word (`-XPOST`) or the next word.
            let letters: Vec<char> = word[1..].chars().collect();
            let mut i = 0;
            while i < letters.len() {
                let c = letters[i];
                i += 1;
                let display = format!("-{c}");
                let long = short_to_long(c).unwrap_or("");
                if takes_value(long) {
                    let attached: String = letters[i..].iter().collect();
                    let value = if attached.is_empty() {
                        words
                            .next()
                            .ok_or_else(|| CurlErrorKind::MissingValue(display.clone()))?
                    } else {
                        attached
                    };
                    flags.apply(&display, long, Some(value));
                    break;
                }
                flags.apply(&display, long, None);
            }
        }
    }
    build(flags, tail, converted, substitutions)
}

fn build(
    flags: Flags,
    tail: Option<String>,
    converted: Vec<String>,
    substitutions: Vec<String>,
) -> Result<CurlImport, CurlErrorKind> {
    let Flags {
        mut urls,
        method: method_flag,
        head,
        get,
        headers: header_rows,
        data,
        json,
        user,
        cookies,
        user_agent,
        referer,
        insecure,
        dropped_body,
        mut ignored,
    } = flags;
    // A shell variable or `$(…)` in the credentials would be encoded as
    // literal text; the flag is dropped instead (spec D8).
    let user = match user {
        Some((display, u))
            if converted.iter().any(|n| u.contains(&format!("{{{{{n}}}}}")))
                || substitutions.iter().any(|s| u.contains(s.as_str())) =>
        {
            ignored.push(format!("{display} {u}"));
            None
        }
        other => other.map(|(_, u)| u),
    };
    let url = urls.pop().ok_or(CurlErrorKind::NoUrl)?;
    ignored.extend(urls);
    // curl assumes http:// for a bare host (spec plan P3).
    let mut url = if url.contains("://") || url.starts_with("{{") {
        url
    } else {
        format!("http://{url}")
    };

    let mut body_text = None;
    if !data.is_empty() {
        let joined = data.join("&");
        if get {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&joined);
        } else {
            body_text = Some(joined);
        }
    }

    // A dropped body still sets curl's method (spec D10), unless `-G`
    // would have sent it in the query.
    let fallback = if head {
        Method::Head
    } else if body_text.is_some() {
        Method::Post
    } else if let Some(m) = dropped_body.filter(|_| !get) {
        m
    } else {
        Method::Get
    };
    let method = match method_flag {
        Some((display, m)) => match Method::ALL.iter().find(|x| x.as_str().eq_ignore_ascii_case(&m)) {
            Some(x) => *x,
            None => {
                ignored.push(format!("{display} {m}"));
                fallback
            }
        },
        None => fallback,
    };
    if body_text.is_some() && !method.sends_body() {
        ignored.push(format!("body ({} sends none)", method.as_str()));
    }

    let mut headers: IndexMap<String, Entry> = IndexMap::new();
    for (name, value) in header_rows {
        add_header(&mut headers, name, value);
    }
    if !cookies.is_empty() {
        add_header(&mut headers, "Cookie".into(), cookies.join("; "));
    }
    let mut derived: Vec<(&str, String)> = Vec::new();
    if let Some(ua) = user_agent {
        derived.push(("User-Agent", ua));
    }
    if let Some(r) = referer {
        derived.push(("Referer", r));
    }
    if let Some(u) = user {
        let creds = if u.contains(':') { u } else { format!("{u}:") };
        derived.push((
            "Authorization",
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(creds)),
        ));
    }
    if json {
        derived.push(("Content-Type", "application/json".into()));
        derived.push(("Accept", "application/json".into()));
    }
    // Spec R3/D5: a body that is not JSON gets curl's own default type;
    // a JSON one is left to postui's auto `application/json`.
    if let Some(text) = &body_text
        && serde_json::from_str::<serde_json::Value>(text).is_err()
    {
        derived.push(("Content-Type", "application/x-www-form-urlencoded".into()));
    }
    for (name, value) in derived {
        if !headers.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            headers.insert(name.to_string(), Entry { value, enabled: true });
        }
    }

    // Only variables that reached the request count as converted: not
    // those that sat in a dropped flag or the pipeline tail.
    let converted: Vec<String> = converted
        .into_iter()
        .filter(|n| {
            let mark = format!("{{{{{n}}}}}");
            url.contains(&mark)
                || headers.values().any(|e| e.value.contains(&mark))
                || body_text.as_ref().is_some_and(|b| b.contains(&mark))
        })
        .collect();
    let substitute_body = body_text
        .as_ref()
        .is_some_and(|b| converted.iter().any(|n| b.contains(&format!("{{{{{n}}}}}"))));
    ignored.extend(substitutions.into_iter().map(|s| format!("{s} kept as text")));
    ignored.extend(tail);

    let suggested_name = suggested_name(method, &url);
    Ok(CurlImport {
        request: HttpRequest {
            name: None,
            method,
            url,
            substitute_body,
            insecure,
            jq: None,
            jq_enabled: true,
            params: IndexMap::new(),
            headers,
            variables: IndexMap::new(),
            body: body_text.map(|text| Body::Json { text }),
        },
        ignored,
        converted,
        suggested_name,
    })
}

/// A repeated header name (case-insensitive) joins onto the first:
/// `; ` for Cookie, `, ` otherwise (HTTP's list rule).
fn add_header(headers: &mut IndexMap<String, Entry>, name: String, value: String) {
    if let Some((_, existing)) = headers.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&name)) {
        let sep = if name.eq_ignore_ascii_case("cookie") { "; " } else { ", " };
        existing.value = format!("{}{sep}{value}", existing.value);
    } else {
        headers.insert(name, Entry { value, enabled: true });
    }
}

fn suggested_name(method: Method, url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    // `user:password@` must never reach a name, a slug or a file name.
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let leaf = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(host);
    let leaf = if leaf.is_empty() { "request" } else { leaf };
    format!("{} {leaf}", method.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curl::to_curl;
    use crate::prepare::{PrepareContext, prepare};

    fn one(text: &str) -> CurlImport {
        let mut batch = parse(text).unwrap();
        assert_eq!(batch.commands.len(), 1, "{batch:?}");
        batch.commands.remove(0)
    }

    fn header<'a>(c: &'a CurlImport, name: &str) -> Option<&'a str> {
        c.request.headers.get(name).map(|e| e.value.as_str())
    }

    fn body(c: &CurlImport) -> Option<&str> {
        c.request.body.as_ref().map(|Body::Json { text }| text.as_str())
    }

    fn err(text: &str) -> CurlError {
        parse(text).unwrap_err()
    }

    #[test]
    fn a_bare_url_is_a_get() {
        let c = one("curl https://api.x/users");
        assert_eq!(c.request.method, Method::Get);
        assert_eq!(c.request.url, "https://api.x/users");
        assert!(c.request.headers.is_empty());
        assert_eq!(body(&c), None);
        assert!(c.ignored.is_empty());
        assert_eq!(c.suggested_name, "GET users");
    }

    #[test]
    fn chrome_bash_sample_imports_exactly() {
        let c = one(concat!(
            "curl 'https://api.example.com/v1/items?page=2' \\\n",
            "  -H 'accept: application/json' \\\n",
            "  -H 'authorization: Bearer abc.def' \\\n",
            "  -H 'content-type: application/json' \\\n",
            "  --data-raw $'{\"name\":\"it\\'s\",\"note\":\"line1\\\\nline2\"}' \\\n",
            "  --compressed"
        ));
        assert_eq!(c.request.method, Method::Post);
        assert_eq!(c.request.url, "https://api.example.com/v1/items?page=2");
        let names: Vec<&str> = c.request.headers.keys().map(String::as_str).collect();
        assert_eq!(names, ["accept", "authorization", "content-type"]);
        assert_eq!(body(&c), Some(r#"{"name":"it's","note":"line1\nline2"}"#));
        assert_eq!(c.ignored, ["--compressed"]);
        assert_eq!(c.suggested_name, "POST items");
    }

    #[test]
    fn firefox_and_safari_samples_import() {
        let f = one("curl 'https://x/api' -X POST -H 'User-Agent: Mozilla/5.0' -H 'Accept: */*' --data-raw '{\"a\":1}'");
        assert_eq!(f.request.method, Method::Post);
        assert_eq!(header(&f, "User-Agent"), Some("Mozilla/5.0"));
        assert_eq!(body(&f), Some(r#"{"a":1}"#));
        let s = one("curl 'https://x/a' \\\n-X 'GET' \\\n-H 'Accept: text/html'");
        assert_eq!(s.request.method, Method::Get);
        assert_eq!(header(&s, "Accept"), Some("text/html"));
    }

    #[test]
    fn a_readme_one_liner_imports() {
        let c = one(r#"curl -X PUT "https://x/items/1" -H "Content-Type: application/json" -d '{"done": true}'"#);
        assert_eq!(c.request.method, Method::Put);
        assert_eq!(body(&c), Some(r#"{"done": true}"#));
        assert_eq!(header(&c, "Content-Type"), Some("application/json"));
        assert_eq!(c.suggested_name, "PUT 1");
    }

    #[test]
    fn bundled_attached_and_equals_forms() {
        let c = one("curl -sSLk -XPATCH -HAccept:x https://x/a");
        assert_eq!(c.request.method, Method::Patch);
        assert!(c.request.insecure);
        assert_eq!(header(&c, "Accept"), Some("x"));
        assert_eq!(c.ignored, ["-s", "-S", "-L"]);
        let d = one("curl --request=DELETE '--header=X-A: 1' --url=https://x/a");
        assert_eq!(d.request.method, Method::Delete);
        assert_eq!(header(&d, "X-A"), Some("1"));
        assert_eq!(d.request.url, "https://x/a");
    }

    #[test]
    fn globoff_is_accepted_silently() {
        for cmd in ["curl -g 'https://x/a?f[s]=1'", "curl --globoff 'https://x/a?f[s]=1'"] {
            let c = one(cmd);
            assert!(c.ignored.is_empty(), "{cmd}: {:?}", c.ignored);
            assert_eq!(c.request.url, "https://x/a?f[s]=1");
        }
    }

    #[test]
    fn a_firefox_ansi_c_body_keeps_its_escapes_and_latin_1_bytes() {
        let c = one(concat!(
            r#"curl 'https://x/api' -X POST --data-raw $'{"name":"Jos"#,
            r"\",
            r#"xe9","q":"say \"hi\" c:\\dir"}'"#
        ));
        assert_eq!(body(&c), Some(r#"{"name":"José","q":"say "hi" c:\dir"}"#));
        assert!(c.ignored.is_empty(), "{:?}", c.ignored);
    }

    #[test]
    fn the_last_url_wins() {
        let c = one("curl https://a/1 https://b/2");
        assert_eq!(c.request.url, "https://b/2");
        assert_eq!(c.ignored, ["https://a/1"]);
    }

    #[test]
    fn an_unsupported_method_falls_back_to_curls_rule() {
        let c = one("curl -X PURGE https://x/a");
        assert_eq!(c.request.method, Method::Get);
        assert_eq!(c.ignored, ["-X PURGE"]);
        assert_eq!(one("curl -X PURGE -d a=1 https://x/a").request.method, Method::Post);
        assert_eq!(one("curl -X post https://x").request.method, Method::Post);
        assert_eq!(one("curl --request PURGE https://x").ignored, ["--request PURGE"]);
    }

    #[test]
    fn head_flag() {
        let c = one("curl -I https://x");
        assert_eq!(c.request.method, Method::Head);
        assert_eq!(c.suggested_name, "HEAD x");
    }

    #[test]
    fn repeated_headers_join_and_cookies_merge() {
        let c = one("curl -H 'Accept: a' -H 'accept: b' -H 'Cookie: c=1' -b 'd=2' https://x");
        assert_eq!(header(&c, "Accept"), Some("a, b"));
        assert_eq!(header(&c, "Cookie"), Some("c=1; d=2"));
        assert_eq!(c.request.headers.len(), 2);
    }

    #[test]
    fn empty_value_and_removal_header_forms() {
        let c = one("curl -H 'X-Empty;' -H 'X-Gone:' https://x");
        assert_eq!(header(&c, "X-Empty"), Some(""));
        assert_eq!(header(&c, "X-Gone"), None);
        assert_eq!(c.ignored, ["-H X-Gone:"]);
    }

    #[test]
    fn data_joins_implies_post_and_non_json_gets_the_form_type() {
        let c = one("curl -d a=1 -d b=2 https://x");
        assert_eq!(c.request.method, Method::Post);
        assert_eq!(body(&c), Some("a=1&b=2"));
        assert_eq!(header(&c, "Content-Type"), Some("application/x-www-form-urlencoded"));
    }

    #[test]
    fn json_without_a_content_type_gets_no_row_d5() {
        let c = one(r#"curl -d '{"a":1}' https://x"#);
        assert_eq!(header(&c, "Content-Type"), None);
        let t = one("curl -H 'Content-Type: text/plain' -d 'hi' https://x");
        assert_eq!(header(&t, "Content-Type"), Some("text/plain"));
    }

    #[test]
    fn file_data_is_ignored_but_data_raw_at_is_literal() {
        let c = one("curl -d @body.json https://x");
        assert_eq!(body(&c), None);
        assert_eq!(c.ignored, ["-d @body.json"]);
        assert_eq!(body(&one("curl --data-raw @x https://x")), Some("@x"));
    }

    #[test]
    fn a_dropped_body_keeps_curls_method_d10() {
        for (cmd, method) in [
            ("curl -d @body.json https://x", Method::Post),
            ("curl --json @b.json https://x", Method::Post),
            ("curl --data-urlencode q@file https://x", Method::Post),
            ("curl -F file=@a.png https://x/upload", Method::Post),
            ("curl --form-string a=b https://x", Method::Post),
            ("curl -T a.txt https://x/put", Method::Put),
            ("curl -X PATCH -F a=b https://x", Method::Patch),
            ("curl -G -d @q.txt https://x", Method::Get),
        ] {
            let c = one(cmd);
            assert_eq!(c.request.method, method, "{cmd}");
            assert_eq!(body(&c), None, "{cmd}");
        }
        assert_eq!(one("curl -T a.txt https://x").ignored, ["-T a.txt"]);
    }

    #[test]
    fn data_urlencode_forms() {
        let c = one("curl --data-urlencode 'q=a b&c' --data-urlencode '=x y' --data-urlencode plain --data-urlencode 'f@file' https://x");
        assert_eq!(body(&c), Some("q=a%20b%26c&x%20y&plain"));
        assert_eq!(c.ignored, ["--data-urlencode f@file"]);
    }

    #[test]
    fn json_flag_sets_both_headers_unless_given() {
        let c = one(r#"curl --json '{"a":1}' https://x"#);
        assert_eq!(c.request.method, Method::Post);
        assert_eq!(header(&c, "Content-Type"), Some("application/json"));
        assert_eq!(header(&c, "Accept"), Some("application/json"));
        let t = one(r#"curl -H 'Accept: text/plain' --json '{}' https://x"#);
        assert_eq!(header(&t, "Accept"), Some("text/plain"));
    }

    #[test]
    fn get_flag_moves_data_to_the_query() {
        let c = one("curl -G https://x/s?a=1 -d q=2 -d r=3");
        assert_eq!(c.request.url, "https://x/s?a=1&q=2&r=3");
        assert_eq!(c.request.method, Method::Get);
        assert_eq!(body(&c), None);
        assert_eq!(one("curl -G https://x/s -d q=2").request.url, "https://x/s?q=2");
    }

    #[test]
    fn user_becomes_basic_auth_unless_authorization_is_given() {
        assert_eq!(header(&one("curl -u alice:secret https://x"), "Authorization"), Some("Basic YWxpY2U6c2VjcmV0"));
        let c = one("curl -u alice:secret -H 'Authorization: Bearer t' https://x");
        assert_eq!(header(&c, "Authorization"), Some("Bearer t"));
    }

    #[test]
    fn a_user_holding_a_variable_is_ignored_not_encoded() {
        let c = one(r#"curl -u "$USER:$PASS" https://x"#);
        assert_eq!(header(&c, "Authorization"), None);
        assert_eq!(c.ignored, ["-u {{USER}}:{{PASS}}"]);
        assert!(c.converted.is_empty());
        let d = one(r#"curl --user "$U:p" https://x"#);
        assert_eq!(d.ignored, ["--user {{U}}:p"]);
        let e = one(r#"curl -u "$U:" -H "X: $T" https://x"#);
        assert_eq!(e.converted, ["T"]);
        assert_eq!(header(&e, "Authorization"), None);
        assert_eq!(header(&one("curl -u a:b https://x"), "Authorization"), Some("Basic YTpi"));
    }

    #[test]
    fn a_user_holding_a_substitution_is_ignored_not_encoded() {
        let c = one(r#"curl -u "me:$(pass show api)" https://x"#);
        assert_eq!(header(&c, "Authorization"), None);
        assert_eq!(c.ignored, ["-u me:$(pass show api)", "$(pass show api) kept as text"]);
        let b = one("curl -u me:`cat pw` https://x");
        assert_eq!(header(&b, "Authorization"), None);
    }

    #[test]
    fn credentials_in_the_url_never_reach_the_name() {
        assert_eq!(one("curl https://admin:s3cret@api.example.com").suggested_name, "GET api.example.com");
        assert_eq!(one("curl https://u:p@h:8080/").suggested_name, "GET h:8080");
        assert_eq!(one("curl https://u:p@h/items").suggested_name, "GET items");
    }

    #[test]
    fn converted_lists_only_variables_in_the_final_request() {
        let c = one(r#"curl -o "$OUT" https://x | jq "$F""#);
        assert!(c.converted.is_empty());
        let d = one(r#"curl -o "$OUT" "$BASE/a" -H "A: $T""#);
        assert_eq!(d.converted, ["BASE", "T"]);
    }

    #[test]
    fn a_cookie_file_is_ignored() {
        let c = one("curl -b cookies.txt https://x");
        assert_eq!(header(&c, "Cookie"), None);
        assert_eq!(c.ignored, ["-b cookies.txt"]);
    }

    #[test]
    fn user_agent_and_referer_yield_to_explicit_headers() {
        let c = one("curl -A agent/1 -e https://ref https://x");
        assert_eq!(header(&c, "User-Agent"), Some("agent/1"));
        assert_eq!(header(&c, "Referer"), Some("https://ref"));
        let m = one("curl -A agent/1 -H 'User-Agent: mine' https://x");
        assert_eq!(header(&m, "User-Agent"), Some("mine"));
    }

    #[test]
    fn a_get_with_data_keeps_the_body_and_says_it_is_not_sent() {
        let c = one("curl -X GET -d a=1 https://x");
        assert_eq!(c.request.method, Method::Get);
        assert_eq!(body(&c), Some("a=1"));
        assert_eq!(c.ignored, ["body (GET sends none)"]);
    }

    #[test]
    fn a_scheme_less_url_gets_http_unless_it_is_a_variable() {
        assert_eq!(one("curl example.com/x").request.url, "http://example.com/x");
        assert_eq!(one("curl '{{base}}/x'").request.url, "{{base}}/x");
        assert_eq!(one("curl $BASE/users").suggested_name, "GET users");
    }

    #[test]
    fn a_body_variable_turns_on_body_substitution() {
        let c = one(r#"curl -d "{\"t\":\"$T\"}" https://x"#);
        assert_eq!(body(&c), Some(r#"{"t":"{{T}}"}"#));
        assert!(c.request.substitute_body);
        assert_eq!(c.converted, ["T"]);
        assert!(!one(r#"curl -H "A: $T" -d '{}' https://x"#).request.substitute_body);
    }

    #[test]
    fn substitutions_and_tails_are_listed_last() {
        let c = one(r#"curl -L -H "X-Date: $(date)" https://a https://b | jq ."#);
        assert_eq!(header(&c, "X-Date"), Some("$(date)"));
        assert_eq!(c.ignored, ["-L", "https://a", "$(date) kept as text", "| jq ."]);
    }

    #[test]
    fn several_commands_skip_the_rest_and_keep_order() {
        let batch = parse("export TOKEN=abc\n# note\ncurl -H \"Authorization: Bearer $TOKEN\" https://x/me\ncurl -X POST https://x/b -d '{}'").unwrap();
        assert_eq!(batch.skipped_lines, 1);
        let names: Vec<&str> = batch.commands.iter().map(|c| c.suggested_name.as_str()).collect();
        assert_eq!(names, ["GET me", "POST b"]);
        assert_eq!(batch.commands[0].converted, ["TOKEN"]);
        assert_eq!(header(&batch.commands[0], "Authorization"), Some("Bearer {{TOKEN}}"));
    }

    #[test]
    fn errors_say_what_and_where() {
        assert_eq!(err("curl https://x -H").kind, CurlErrorKind::MissingValue("-H".into()));
        assert_eq!(err("curl https://x -H").command, None);
        assert_eq!(err("curl -k").kind, CurlErrorKind::NoUrl);
        assert_eq!(err("wget https://x").kind, CurlErrorKind::NotCurl);
        assert_eq!(err("").kind, CurlErrorKind::NotCurl);
        assert_eq!(err("curl ^\"https://x^\" ^\n -k").kind, CurlErrorKind::CmdStyle);
        let e = err("curl https://x/a\ncurl -H");
        assert_eq!((e.command, e.kind.clone()), (Some(2), CurlErrorKind::MissingValue("-H".into())));
        assert_eq!(e.to_string(), "command 2: -H needs a value");
        assert_eq!(err("curl a\ncurl 'b").to_string(), "command 2: unclosed quote");
        assert_eq!(err("hello").to_string(), "clipboard doesn't hold a curl command");
        assert_eq!(
            err("curl ^\"https://x^\" ^\n -k").to_string(),
            "this is a Windows cmd command \u{2014} copy as cURL (bash) instead"
        );
        assert_eq!(err("curl -k").to_string(), "no URL in the curl command");
    }

    /// Spec §7: exporting then importing gives back what Send would send.
    #[test]
    fn export_then_import_round_trips() {
        let cases = [
            "url = \"https://x/a?q=it's%20$HOME\"\n[params]\npage = \"2\"\n",
            "url = \"https://x/a?q=it's%20$HOME\"\n",
            "method = \"POST\"\nurl = \"https://x/users\"\n[headers]\nX-Note = \"it's $5\"\n[body]\ntype = \"json\"\ntext = \"{\\n  \\\"a\\\": \\\"b'c\\\"\\n}\"\n",
            "method = \"PUT\"\nurl = \"https://x/1\"\n[headers]\nX-Empty = \"\"\n",
            "method = \"HEAD\"\nurl = \"https://x\"\n",
            "method = \"DELETE\"\nurl = \"https://x/1\"\ninsecure = true\n",
            "url = \"https://x/items?page[size]=10&q={a,b}\"\n",
            "method = \"POST\"\nurl = \"https://x\"\n[body]\ntype = \"json\"\ntext = \"@not-a-file\"\n",
            "method = \"PATCH\"\nurl = \"https://x\"\n[headers]\nContent-Type = \"text/plain\"\n[body]\ntype = \"json\"\ntext = \"plain words\"\n",
        ];
        for toml in cases {
            let req = HttpRequest::from_toml_str(toml).unwrap();
            let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
            let back = one(&to_curl(&p));
            assert_eq!(back.request.method, p.method, "{toml}");
            assert_eq!(back.request.url, p.url, "{toml}");
            let mut sent = p.headers.clone();
            sent.sort();
            let mut got: Vec<(String, String)> =
                back.request.headers.iter().map(|(k, e)| (k.clone(), e.value.clone())).collect();
            got.sort();
            assert_eq!(got, sent, "{toml}");
            assert_eq!(body(&back), p.body.as_deref(), "{toml}");
            assert_eq!(back.request.insecure, p.insecure, "{toml}");
            assert!(back.ignored.is_empty(), "{toml}: {:?}", back.ignored);
        }
    }
}
