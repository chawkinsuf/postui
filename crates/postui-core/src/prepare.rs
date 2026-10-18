use crate::model::{Body, Entry, HttpRequest, Method};
use crate::varmodel;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRequest {
    pub method: Method,
    pub url: String,
    /// `url` with secret variable values rendered as [`SECRET_MASK`] — the
    /// version screens show for the sent request; never what goes on the
    /// wire.
    pub display_url: String,
    /// The headers the request itself sets, in wire order: the sent
    /// [`header_rows`] other than the client's. What the request is built
    /// from (and what curl export writes). Each row has its masked twin in
    /// `display_name`/`display`, the version screens show.
    pub headers: Vec<HeaderRow>,
    /// What the HTTP client adds on its own after `headers` (`Accept`,
    /// `Host`, `Content-Length`), for the screens that show the whole send
    /// (the response pane's `sent` section) — never put on the request.
    pub client_headers: Vec<HeaderRow>,
    pub body: Option<String>,
    /// Skip TLS certificate verification for this send (the request's
    /// `insecure` flag, carried through so the transport can pick a client).
    pub insecure: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrepareWarning {
    ParamOverridesUrl { key: String },
    DuplicateDefaultHeader { name: String },
}

impl fmt::Display for PrepareWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrepareWarning::ParamOverridesUrl { key } => {
                write!(f, "query param `{}` in [params] overrides the one in the URL", key)
            }
            PrepareWarning::DuplicateDefaultHeader { name } => {
                write!(f, "duplicate default header '{}'", name)
            }
        }
    }
}

/// Everything `prepare` needs beyond the request itself: fully-resolved
/// variables (env over default) and the project's default headers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrepareContext {
    pub vars: IndexMap<String, String>,
    pub default_headers: IndexMap<String, Entry>,
    /// Per-name resolution metadata from `varmodel::resolve_env`, used to
    /// distinguish *why* a name is unresolved (needs a selection, missing
    /// secret, or simply undefined) when it doesn't appear in `vars`.
    pub meta: IndexMap<String, varmodel::VarMeta>,
    /// The active environment's TLS policy (`[environment.<slug>] tls`),
    /// which overrides the request's own `insecure` flag when set. The
    /// request file is never touched — switching environments restores
    /// its setting.
    pub tls_override: Option<crate::project::TlsPolicy>,
}

/// Why a `{{name}}` token failed to resolve, for `PrepareError::Unresolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedCause {
    Undefined,
    NeedsSelection,
    MissingSecret,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrepareError {
    Unresolved(BTreeMap<String, UnresolvedCause>),
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrepareError::Unresolved(causes) => {
                let mut undefined = Vec::new();
                let mut needs_selection = Vec::new();
                let mut missing_secret = Vec::new();
                for (name, cause) in causes {
                    match cause {
                        UnresolvedCause::Undefined => undefined.push(name.clone()),
                        UnresolvedCause::NeedsSelection => needs_selection.push(name.clone()),
                        UnresolvedCause::MissingSecret => missing_secret.push(name.clone()),
                    }
                }
                let mut groups = Vec::new();
                if !undefined.is_empty() {
                    groups.push(format!("unresolved: {}", undefined.join(", ")));
                }
                if !needs_selection.is_empty() {
                    groups.push(format!("need a selection: {}", needs_selection.join(", ")));
                }
                if !missing_secret.is_empty() {
                    groups.push(format!("missing secret: {}", missing_secret.join(", ")));
                }
                write!(f, "{}", groups.join(" · "))
            }
        }
    }
}

impl std::error::Error for PrepareError {}

/// What a secret's value renders as on screen (a header row's `display`,
/// `PreparedRequest::display_url`).
pub const SECRET_MASK: &str = "●●●●●●";

/// Request `[variables]` overlaid on the context's resolved vars at highest
/// precedence: enabled entries only, on a clone so the context is untouched.
fn overlaid_vars(req: &HttpRequest, ctx: &PrepareContext) -> IndexMap<String, String> {
    let mut vars = ctx.vars.clone();
    for (k, e) in &req.variables {
        if e.enabled {
            vars.insert(k.clone(), e.value.clone());
        }
    }
    vars
}

/// The one substitution both `prepare` and `header_rows` go through:
/// replaces every resolvable `{{name}}`, leaves the rest verbatim while
/// collecting their names into `unresolved`, and renders the value of any
/// name in `masked` as [`SECRET_MASK`] instead.
fn substitute_masked(
    text: &str,
    vars: &IndexMap<String, String>,
    masked: &BTreeSet<String>,
    unresolved: &mut BTreeSet<String>,
) -> String {
    if masked.is_empty() {
        return crate::vars::substitute(text, vars, unresolved);
    }
    let tokens = crate::vars::find_tokens(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for t in tokens {
        out.push_str(&text[last..t.start]);
        match vars.get(&t.name) {
            Some(_) if masked.contains(&t.name) => out.push_str(SECRET_MASK),
            Some(v) => out.push_str(v),
            None => {
                unresolved.insert(t.name.clone());
                out.push_str(&text[t.start..t.end]);
            }
        }
        last = t.end;
    }
    out.push_str(&text[last..]);
    out
}

/// One substitution pass of the final URL: substitutes the URL and the
/// enabled `[params]`, then merges the params into the query string
/// (`[params]` overriding URL pairs, one warning per override). Shared by
/// `prepare`'s real pass and its masked `display_url` pass so the two can
/// never assemble differently.
fn assemble_url(
    req: &HttpRequest,
    sub: &mut dyn FnMut(&str) -> String,
    warnings: &mut Vec<PrepareWarning>,
) -> String {
    let subbed_url = sub(&req.url);
    let enabled: Vec<(String, String)> = req
        .params
        .iter()
        .filter(|(_, e)| e.enabled)
        .map(|(k, e)| (sub(k), sub(&e.value)))
        .collect();
    if enabled.is_empty() {
        return subbed_url;
    }
    // A fragment is never part of the query: split it off first, or `?`
    // inside it would be taken for the query and the whole `#…` would be
    // percent-encoded into the last parameter's value.
    let (without_fragment, fragment) = match subbed_url.split_once('#') {
        Some((u, f)) => (u.to_string(), Some(f.to_string())),
        None => (subbed_url.clone(), None),
    };
    let (base, query) = match without_fragment.split_once('?') {
        Some((b, q)) => (b.to_string(), q.to_string()),
        None => (without_fragment.clone(), String::new()),
    };
    // (key, value) pairs; URL pairs first, in order, before the
    // `[params]` table's entries are merged in below.
    let mut pairs: Vec<(String, String)> =
        form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    for (k, v) in &enabled {
        let existing = pairs.iter().position(|(pk, _)| pk == k);
        if let Some(i) = existing {
            warnings.push(PrepareWarning::ParamOverridesUrl { key: k.clone() });
            pairs.retain(|(pk, _)| pk != k);
            pairs.insert(i.min(pairs.len()), (k.clone(), v.clone()));
        } else {
            pairs.push((k.clone(), v.clone()));
        }
    }
    let qs = form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
    match fragment {
        Some(f) => format!("{base}?{qs}#{f}"),
        None => format!("{base}?{qs}"),
    }
}

pub fn prepare(
    req: &HttpRequest,
    ctx: &PrepareContext,
) -> Result<(PreparedRequest, Vec<PrepareWarning>), PrepareError> {
    let vars = overlaid_vars(req, ctx);

    let mut missing = BTreeSet::new();
    let no_mask = BTreeSet::new();
    let mut sub = |s: &str| substitute_masked(s, &vars, &no_mask, &mut missing);

    let mut warnings = Vec::new();
    let url = assemble_url(req, &mut sub, &mut warnings);
    // The same assembly again with secret values masked — what screens
    // show for the sent request (`PreparedRequest::display_url`). Its
    // warnings are duplicates of the real pass's, so they're dropped.
    let secret_names = secret_names(ctx);
    let display_url = if secret_names.is_empty() {
        url.clone()
    } else {
        let mut ignored = BTreeSet::new();
        let masked = assemble_url(
            req,
            &mut |s| substitute_masked(s, &vars, &secret_names, &mut ignored),
            &mut Vec::new(),
        );
        // A mask that landed in the query string got percent-encoded by
        // the serializer; put the readable glyphs back — this is a display
        // string, not what goes on the wire.
        let encoded_mask: String =
            form_urlencoded::byte_serialize(SECRET_MASK.as_bytes()).collect();
        masked.replace(&encoded_mask, SECRET_MASK)
    };

    let body = sent_body(req).map(|b| if req.substitute_body { sub(b) } else { b.to_string() });

    // Only what goes on the wire has to resolve: a row that isn't sent
    // (an overridden default, a row naming a client-owned header) never
    // blocks the send, whatever its tokens. A row the client-owned rule
    // drops is left without a warning on purpose: the Headers tab marks it
    // as an error where it is written, and the send goes as the user asked
    // it not to fail (spec 2026-10-18 R3a/R3b; a send-time notice was
    // weighed in review and declined).
    let (client_headers, headers): (Vec<HeaderRow>, Vec<HeaderRow>) = rows(req, ctx, &mut warnings)
        .into_iter()
        .filter(HeaderRow::sent)
        .partition(|r| r.origin == HeaderOrigin::Client);
    missing.extend(headers.iter().flat_map(|r| r.unresolved.iter().cloned()));

    if !missing.is_empty() {
        let causes = missing
            .into_iter()
            .map(|name| {
                let cause = match ctx.meta.get(&name) {
                    Some(varmodel::VarMeta::NeedsSelection) => UnresolvedCause::NeedsSelection,
                    Some(varmodel::VarMeta::MissingSecret) => UnresolvedCause::MissingSecret,
                    _ => UnresolvedCause::Undefined,
                };
                (name, cause)
            })
            .collect();
        return Err(PrepareError::Unresolved(causes));
    }

    Ok((
        PreparedRequest {
            method: req.method,
            url,
            display_url,
            headers,
            client_headers,
            body,
            insecure: match ctx.tls_override {
                Some(crate::project::TlsPolicy::Verify) => false,
                Some(crate::project::TlsPolicy::Insecure) => true,
                None => req.insecure,
            },
        },
        warnings,
    ))
}

/// Where a header row comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderOrigin {
    /// An enabled row of the request's own table; `row` is its index in
    /// that table (disabled rows counted), so the editor can point at it.
    Request { row: usize },
    /// A project `[default_headers]` row.
    Default,
    /// The `Content-Type: application/json` added for a body.
    ContentType,
    /// Added by the HTTP client itself (`Accept`, `Host`,
    /// `Content-Length`): listed so the screens show the whole send, but
    /// never put on the request by us.
    Client,
}

/// One header of a send, as [`header_rows`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderRow {
    /// The real name; an unresolved `{{token}}` keeps its braces.
    pub name: String,
    /// `name` with secret values rendered as [`SECRET_MASK`].
    pub display_name: String,
    /// The real value; an unresolved `{{token}}` keeps its braces.
    pub value: String,
    /// `value` with secret values rendered as [`SECRET_MASK`]: with
    /// `display_name`, what screens show while secrets are hidden.
    pub display: String,
    pub origin: HeaderOrigin,
    /// Why the row doesn't go on the wire, or `None` when it does. A row
    /// that isn't sent is still listed, so screens can say why.
    pub unsent: Option<Unsent>,
    /// Names of `{{tokens}}` in the name or value that didn't resolve.
    pub unresolved: Vec<String>,
}

impl HeaderRow {
    /// Whether the row goes on the wire.
    pub fn sent(&self) -> bool {
        self.unsent.is_none()
    }
}

/// Why a listed [`HeaderRow`] isn't sent: recorded where the rule is
/// applied, so no screen has to work it out again from the row's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsent {
    /// A row that goes out names it instead: the request's own (enabled or
    /// disabled) over a default, or a sent row over an auto or client row.
    Replaced,
    /// A default whose name an earlier default already sends.
    Duplicate,
    /// It names a [`CLIENT_OWNED`] header (R3a). `client_sends`: whether
    /// the client sends its own row for it in this send — not without a
    /// body for `Content-Length`, nor while the host is a `{{token}}`.
    ClientOwned { header: &'static str, client_sends: bool },
}

/// Headers the HTTP client sets from the URL and the body, and which
/// nothing may override (spec 2026-10-18 R3a): a request or default row
/// with one of these names is listed but never sent, and the client's own
/// row always is.
pub const CLIENT_OWNED: [&str; 2] = ["Host", "Content-Length"];

/// The [`CLIENT_OWNED`] header `name` names, in its usual spelling.
pub fn client_owned(name: &str) -> Option<&'static str> {
    CLIENT_OWNED.into_iter().find(|owned| name.eq_ignore_ascii_case(owned))
}

/// Every header of `req`'s send, in wire order — enabled defaults,
/// enabled request rows, the auto `Content-Type`, then the client's
/// `Accept`, `Host` and `Content-Length` — whether sent or not. The one
/// place a send's headers are worked out: [`prepare`] keeps the sent
/// rows, and the editor's auto section draws the rest of the story.
/// Never errors: unresolved tokens stay literal and are listed per row.
pub fn header_rows(req: &HttpRequest, ctx: &PrepareContext) -> Vec<HeaderRow> {
    rows(req, ctx, &mut Vec::new())
}

/// [`header_rows`], also reporting duplicate defaults to `warnings`.
fn rows(
    req: &HttpRequest,
    ctx: &PrepareContext,
    warnings: &mut Vec<PrepareWarning>,
) -> Vec<HeaderRow> {
    let vars = overlaid_vars(req, ctx);
    let secrets = secret_names(ctx);
    let no_mask = BTreeSet::new();
    let quiet = |s: &str| substitute_masked(s, &vars, &no_mask, &mut BTreeSet::new());
    // The screens' version of `text`: the real one when no secret is in
    // play, which is most projects, on every frame the Headers tab shows.
    let masked = |text: &str, real: &String| {
        if secrets.is_empty() {
            real.clone()
        } else {
            substitute_masked(text, &vars, &secrets, &mut BTreeSet::new())
        }
    };
    // What the client derives on its own, worked out first so a row naming
    // a client-owned header can say whether the client sends one.
    let url = quiet(&req.url);
    let host = authority(&url);
    let body = sent_body(req);
    let client_sends = |header: &str| match header {
        "Host" => host.is_some(),
        _ => body.is_some(),
    };
    // Substitutes name and value together, so a row reports the
    // unresolved tokens of both halves.
    let row = |name: &str, value: &str, origin: HeaderOrigin| {
        let mut unresolved = BTreeSet::new();
        let real_name = substitute_masked(name, &vars, &no_mask, &mut unresolved);
        let real = substitute_masked(value, &vars, &no_mask, &mut unresolved);
        HeaderRow {
            unsent: client_owned(&real_name)
                .map(|header| Unsent::ClientOwned { header, client_sends: client_sends(header) }),
            display_name: masked(name, &real_name),
            name: real_name,
            display: masked(value, &real),
            value: real,
            origin,
            unresolved: unresolved.into_iter().collect(),
        }
    };
    let fixed = |name: &str, value: String, display: String, unsent: Option<Unsent>| HeaderRow {
        name: name.to_string(),
        display_name: name.to_string(),
        value,
        display,
        origin: HeaderOrigin::Client,
        unsent,
        unresolved: Vec::new(),
    };

    let mut rows = Vec::new();

    // Defaults merge UNDER the request's rows: a default is not sent when
    // the request has any row, enabled or disabled, whose name matches
    // case-insensitively after substitution (`{{h}}` with h = Accept is an
    // Accept row). Of two defaults resolving to the same lowercase name,
    // the first is sent and the rest are reported — unless the name is
    // client-owned, when none of them is sent and there is no choice to
    // report.
    let request_names: Vec<String> = req.headers.keys().map(|k| quiet(k)).collect();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for (k, e) in ctx.default_headers.iter().filter(|(_, e)| e.enabled) {
        let mut r = row(k, &e.value, HeaderOrigin::Default);
        let lower = r.name.to_ascii_lowercase();
        // A client-owned default is already unsent, whatever else names it.
        if r.sent() && request_names.iter().any(|n| n.eq_ignore_ascii_case(&r.name)) {
            r.unsent = Some(Unsent::Replaced);
        } else if r.sent() && !claimed.insert(lower.clone()) {
            warnings.push(PrepareWarning::DuplicateDefaultHeader { name: lower });
            r.unsent = Some(Unsent::Duplicate);
        }
        rows.push(r);
    }
    for (i, (k, e)) in req.headers.iter().enumerate().filter(|(_, (_, e))| e.enabled) {
        rows.push(row(k, &e.value, HeaderOrigin::Request { row: i }));
    }

    // An auto or client row stands aside for a sent row naming it.
    let replaced = |rows: &[HeaderRow], name: &str| {
        rows.iter()
            .any(|r| r.sent() && r.name.eq_ignore_ascii_case(name))
            .then_some(Unsent::Replaced)
    };
    if body.is_some() {
        let json = "application/json".to_string();
        let unsent = replaced(&rows, "content-type");
        rows.push(HeaderRow {
            origin: HeaderOrigin::ContentType,
            ..fixed("Content-Type", json.clone(), json, unsent)
        });
    }

    // The client's own rows, in the order it sends them. `Accept: */*` is
    // its default unless a sent row names `accept`.
    let unsent = replaced(&rows, "accept");
    rows.push(fixed("Accept", "*/*".into(), "*/*".into(), unsent));
    // `Host` is the URL's authority as the client derives it, real and
    // masked. A URL whose host is still a `{{token}}` has none yet.
    if let Some(host) = host {
        let display = if secrets.is_empty() {
            host.clone()
        } else {
            let masked = substitute_masked(&req.url, &vars, &secrets, &mut BTreeSet::new());
            // Never the real host: it may hold the secret. A masked URL
            // always has a host where the real one does, so this is only
            // the safe answer for a case that doesn't arise.
            authority(&masked).unwrap_or_else(|| SECRET_MASK.to_string())
        };
        rows.push(fixed("Host", host, display, None));
    }
    if let Some(body) = body {
        // The byte length of what goes on the wire: the substituted body
        // when the request opts in, never the masked one.
        let len = if req.substitute_body { quiet(body).len() } else { body.len() };
        rows.push(fixed("Content-Length", len.to_string(), len.to_string(), None));
    }
    rows
}

/// The body a send carries: none for a method that sends none, even when
/// the request keeps body text for when the method switches back.
fn sent_body(req: &HttpRequest) -> Option<&str> {
    req.method.sends_body().then(|| req.body.as_ref().map(|Body::Json { text }| text.as_str()))?
}

/// The names of the secret variables, whose values screens mask.
fn secret_names(ctx: &PrepareContext) -> BTreeSet<String> {
    ctx.meta
        .iter()
        .filter(|(_, m)| **m == varmodel::VarMeta::Secret)
        .map(|(name, _)| name.clone())
        .collect()
}

/// What [`authority`] parses in place of the secret mask: the parser
/// would punycode the mask's glyphs into `xn--…`, but leaves a plain
/// lowercase label as it is, so the mask can be put back afterwards.
const MASK_STAND_IN: &str = "postui-secret-mask";

/// The `Host` value the client derives from `url`: the host, lowercased
/// (and punycoded), plus the port when it isn't the scheme's default — no
/// userinfo. A masked URL goes through the same rule, its mask kept.
/// `None` while the host is empty or still holds a `{{token}}`. A URL the
/// parser refuses keeps its host and port as written (the send itself
/// will report that URL); so does a masked one whose mask the parser
/// can't take in place of the secret (a secret port, a label pushed past
/// 63 characters) — still masked, just not in the client's exact form.
/// That cosmetic gap is accepted rather than chased further.
fn authority(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let segment = after_scheme.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = segment.rsplit_once('@').map_or(segment, |(_, host_port)| host_port);
    if host_port.is_empty() || host_port.contains("{{") {
        return None;
    }
    // Only a masked URL makes the round trip, so a real host that happens
    // to hold the stand-in is left as it is.
    let masked = url.contains(SECRET_MASK);
    let parsed = if masked {
        url::Url::parse(&url.replace(SECRET_MASK, MASK_STAND_IN)).ok()
    } else {
        url::Url::parse(url).ok()
    };
    let Some(host) = parsed.as_ref().and_then(|u| u.host_str()) else {
        return Some(host_port.to_string());
    };
    let host = if masked { host.replace(MASK_STAND_IN, SECRET_MASK) } else { host.to_string() };
    Some(match parsed.and_then(|u| u.port()) {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use indexmap::IndexMap;

    /// The `(name, value)` pairs the request itself puts on the wire.
    fn wire(p: &PreparedRequest) -> Vec<(String, String)> {
        p.headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect()
    }

    fn ctx(vars: &[(&str, &str)], defaults: &[(&str, &str, bool)]) -> PrepareContext {
        PrepareContext {
            vars: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            default_headers: defaults
                .iter()
                .map(|(k, v, en)| (k.to_string(), Entry { value: v.to_string(), enabled: *en }))
                .collect(),
            meta: IndexMap::new(),
            tls_override: None,
        }
    }

    #[test]
    fn substitutes_url_params_and_headers() {
        let mut r = base("{{base}}/users");
        r.params.insert("{{pkey}}".into(), on("{{pval}}"));
        r.headers.insert("x-{{h}}".into(), on("{{hv}}"));
        let c = ctx(
            &[
                ("base", "http://x.test"),
                ("pkey", "id"),
                ("pval", "7"),
                ("h", "trace"),
                ("hv", "on"),
            ],
            &[],
        );
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.url, "http://x.test/users?id=7");
        assert!(wire(&p).contains(&("x-trace".into(), "on".into())));
    }

    #[test]
    fn get_and_head_send_no_body() {
        // The stored body text survives on the request (it's still there
        // when the method switches back) but nothing goes on the wire.
        for m in [Method::Get, Method::Head] {
            let mut r = base("http://x.test");
            r.method = m;
            r.body = Some(Body::Json { text: "{}".into() });
            let c = ctx(&[], &[]);
            let (p, _) = prepare(&r, &c).unwrap();
            assert_eq!(p.body, None, "{m:?} must not send a body");
            assert!(
                !p.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type")),
                "{m:?} must not auto-add Content-Type"
            );
            let rows = header_rows(&r, &c);
            assert!(
                !rows.iter().any(|r| r.name == "Content-Type" || r.name == "Content-Length"),
                "{m:?} computed headers carry no body rows"
            );
        }
    }

    #[test]
    fn body_substitution_is_opt_in() {
        let mut r = base("http://x.test");
        r.method = Method::Post;
        r.body = Some(Body::Json { text: r#"{"t": "{{tok}}"}"#.into() });
        let c = ctx(&[("tok", "abc")], &[]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.body.as_deref(), Some(r#"{"t": "{{tok}}"}"#), "flag off: literal braces");
        r.substitute_body = true;
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.body.as_deref(), Some(r#"{"t": "abc"}"#));
    }

    #[test]
    fn unresolved_variables_error_and_body_tokens_only_count_when_opted_in() {
        let mut r = base("http://x.test/{{gone}}");
        r.method = Method::Post;
        r.body = Some(Body::Json { text: "{{also_gone}}".into() });
        let err = prepare(&r, &PrepareContext::default()).unwrap_err();
        let PrepareError::Unresolved(causes) = err;
        assert_eq!(
            causes.into_keys().collect::<Vec<_>>(),
            vec!["gone".to_string()],
            "body ignored while flag off"
        );
        r.substitute_body = true;
        let PrepareError::Unresolved(causes) = prepare(&r, &PrepareContext::default()).unwrap_err();
        assert_eq!(causes.len(), 2);
    }

    #[test]
    fn default_headers_merge_override_and_suppress() {
        let mut r = base("http://x.test");
        r.headers.insert("Accept".into(), on("text/plain")); // overrides (case-insensitive)
        r.headers.insert("X-Trace".into(), off("ignored")); // disabled row suppresses inherited
        let c = ctx(
            &[],
            &[("accept", "application/json", true), ("x-trace", "1", true), ("x-off", "0", false)],
        );
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![("Accept".to_string(), "text/plain".to_string())],
            "override kept in request position; suppressed + disabled defaults dropped"
        );
    }

    #[test]
    fn default_header_values_are_substituted_too() {
        let r = base("http://x.test");
        let c = ctx(&[("tok", "abc")], &[("authorization", "Bearer {{tok}}", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert!(wire(&p).contains(&("authorization".into(), "Bearer abc".into())));
    }

    #[test]
    fn default_header_suppression_compares_names_after_substitution() {
        let mut r = base("http://x.test");
        r.headers.insert("x-api".into(), on("literal"));
        let c = ctx(&[("t", "Api")], &[("X-{{t}}", "default-value", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![("x-api".to_string(), "literal".to_string())],
            "substituted default name X-Api matches request header x-api case-insensitively"
        );
    }

    #[test]
    fn templated_request_header_name_suppresses_the_same_named_default() {
        // The request row's NAME is a template: after substitution it is
        // the default's name, so the request's value must win on the wire
        // (headers are appended, so a leftover default would be sent
        // first and a server would take it over the user's override).
        let mut r = base("http://x.test");
        r.headers.insert("{{auth_hdr}}".into(), on("Bearer req"));
        let c = ctx(&[("auth_hdr", "Authorization")], &[("Authorization", "Bearer default", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![("Authorization".to_string(), "Bearer req".to_string())],
            "the substituted request name suppresses the default"
        );
    }

    #[test]
    fn suppression_probe_does_not_report_a_disabled_rows_unresolved_name() {
        let mut r = base("http://x.test");
        r.headers.insert("{{nope}}".into(), off("ignored"));
        let c = ctx(&[], &[("X-Default", "d", true)]);
        let (p, _) = prepare(&r, &c).expect("a disabled row's template is never unresolved");
        assert_eq!(wire(&p), vec![("X-Default".to_string(), "d".to_string())]);
    }

    #[test]
    fn header_rows_mark_a_default_unsent_by_a_templated_request_name() {
        let mut req = base("http://x.test");
        req.headers.insert("{{auth_hdr}}".into(), on("Bearer req"));
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("auth_hdr".into(), "Authorization".into());
        ctx.default_headers.insert("Authorization".into(), on("Bearer default"));
        let rows = header_rows(&req, &ctx);
        assert!(rows.iter().any(|r| r.origin == HeaderOrigin::Default && !r.sent()), "{rows:?}");
    }

    #[test]
    fn default_header_suppression_after_substitution_honors_disabled_row() {
        let mut r = base("http://x.test");
        r.headers.insert("x-api".into(), off("ignored"));
        let c = ctx(&[("t", "Api")], &[("X-{{t}}", "default-value", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert!(
            wire(&p).is_empty(),
            "disabled request row still suppresses the substituted default entirely"
        );
    }

    #[test]
    fn duplicate_default_headers_after_substitution_keep_first_and_warn() {
        let mut defaults = IndexMap::new();
        defaults.insert("X-One".to_string(), on("first"));
        defaults.insert("x-one".to_string(), on("second"));
        let c = PrepareContext {
            vars: IndexMap::new(),
            default_headers: defaults,
            meta: IndexMap::new(),
            tls_override: None,
        };
        let (p, warns) = prepare(&base("http://x.test"), &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![("X-One".to_string(), "first".to_string())],
            "first duplicate wins"
        );
        assert_eq!(
            warns,
            vec![PrepareWarning::DuplicateDefaultHeader { name: "x-one".to_string() }]
        );
    }

    #[test]
    fn request_variable_overlay_beats_context_vars() {
        let mut r = base("http://x.test/{{name}}");
        r.variables.insert("name".into(), on("request-scoped"));
        let c = ctx(&[("name", "project-scoped")], &[]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.url, "http://x.test/request-scoped");
    }

    #[test]
    fn disabled_request_variable_does_not_resolve() {
        let mut r = base("http://x.test/{{name}}");
        r.variables.insert("name".into(), off("request-scoped"));
        let err = prepare(&r, &PrepareContext::default()).unwrap_err();
        let PrepareError::Unresolved(causes) = err;
        assert_eq!(causes.get("name"), Some(&UnresolvedCause::Undefined));
    }

    #[test]
    fn unresolved_causes_mapped_from_context_meta() {
        let r = base("http://x.test/{{gone}}/{{user}}/{{api_key}}");
        let mut c = PrepareContext::default();
        c.meta.insert("user".into(), varmodel::VarMeta::NeedsSelection);
        c.meta.insert("api_key".into(), varmodel::VarMeta::MissingSecret);
        let PrepareError::Unresolved(causes) = prepare(&r, &c).unwrap_err();
        assert_eq!(causes.get("gone"), Some(&UnresolvedCause::Undefined));
        assert_eq!(causes.get("user"), Some(&UnresolvedCause::NeedsSelection));
        assert_eq!(causes.get("api_key"), Some(&UnresolvedCause::MissingSecret));
    }

    #[test]
    fn display_groups_unresolved_by_cause() {
        let mut causes = BTreeMap::new();
        causes.insert("a".to_string(), UnresolvedCause::Undefined);
        causes.insert("b".to_string(), UnresolvedCause::Undefined);
        causes.insert("user".to_string(), UnresolvedCause::NeedsSelection);
        causes.insert("api_key".to_string(), UnresolvedCause::MissingSecret);
        let err = PrepareError::Unresolved(causes);
        assert_eq!(
            err.to_string(),
            "unresolved: a, b · need a selection: user · missing secret: api_key"
        );
    }

    #[test]
    fn display_omits_empty_cause_groups() {
        let mut causes = BTreeMap::new();
        causes.insert("only".to_string(), UnresolvedCause::NeedsSelection);
        let err = PrepareError::Unresolved(causes);
        assert_eq!(err.to_string(), "need a selection: only");
    }

    fn base(url: &str) -> HttpRequest {
        HttpRequest {
            name: None,
            method: Method::Get,
            url: url.into(),
            substitute_body: false,
            insecure: false,
            jq: None,
            jq_enabled: true,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            body: None,
        }
    }
    fn on(v: &str) -> Entry {
        Entry { value: v.into(), enabled: true }
    }
    fn off(v: &str) -> Entry {
        Entry { value: v.into(), enabled: false }
    }

    #[test]
    fn insecure_flag_is_carried_onto_the_prepared_request() {
        let r = base("https://x.test");
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert!(!p.insecure, "verifying is the default");

        let mut r = base("https://x.test");
        r.insecure = true;
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert!(p.insecure);
    }

    #[test]
    fn environment_tls_override_beats_the_request_flag() {
        use crate::project::TlsPolicy;
        let mut r = base("https://x.test");
        let mut ctx = PrepareContext::default();

        // Force verify: a request flagged insecure still verifies.
        r.insecure = true;
        ctx.tls_override = Some(TlsPolicy::Verify);
        let (p, _) = prepare(&r, &ctx).unwrap();
        assert!(!p.insecure, "forced verify overrides insecure = true");

        // Force insecure: an unflagged request skips verification.
        r.insecure = false;
        ctx.tls_override = Some(TlsPolicy::Insecure);
        let (p, _) = prepare(&r, &ctx).unwrap();
        assert!(p.insecure, "forced insecure overrides insecure = false");

        // No override: the request's own flag.
        ctx.tls_override = None;
        let (p, _) = prepare(&r, &ctx).unwrap();
        assert!(!p.insecure);
        r.insecure = true;
        let (p, _) = prepare(&r, &ctx).unwrap();
        assert!(p.insecure);
    }

    #[test]
    fn merges_enabled_params_into_query_encoding_values() {
        let mut r = base("https://x.test/path");
        r.params.insert("q".into(), on("a b&c"));
        r.params.insert("skip".into(), off("nope"));
        let (p, warns) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/path?q=a+b%26c");
        assert!(warns.is_empty());
    }

    #[test]
    fn a_fragment_stays_a_fragment_when_params_are_merged() {
        let mut r = base("https://x.test/p?id=1#sec?tion");
        r.params.insert("q".into(), on("v"));
        let (p, warns) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/p?id=1&q=v#sec?tion");
        assert!(warns.is_empty());
        let mut r = base("https://x.test/p#top");
        r.params.insert("q".into(), on("v"));
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/p?q=v#top");
    }

    #[test]
    fn params_table_wins_over_url_query_with_warning() {
        let mut r = base("https://x.test/p?id=1&keep=y");
        r.params.insert("id".into(), on("2"));
        let (p, warns) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/p?id=2&keep=y");
        assert_eq!(warns, vec![PrepareWarning::ParamOverridesUrl { key: "id".into() }]);
    }

    #[test]
    fn url_literal_duplicates_are_kept_verbatim() {
        let (p, warns) =
            prepare(&base("https://x.test/p?id=1&id=2"), &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/p?id=1&id=2");
        assert!(warns.is_empty(), "user-typed duplicates pass through untouched");
    }

    #[test]
    fn url_without_params_table_is_untouched() {
        let (p, _) =
            prepare(&base("https://x.test/p?a=%20weird&b"), &PrepareContext::default()).unwrap();
        assert_eq!(
            p.url, "https://x.test/p?a=%20weird&b",
            "no table entries: never rewrite the query"
        );
    }

    #[test]
    fn headers_filter_disabled_and_json_body_auto_adds_content_type() {
        let mut r = base("https://x.test");
        r.method = Method::Post;
        r.headers.insert("A".into(), on("1"));
        r.headers.insert("B".into(), off("2"));
        r.body = Some(Body::Json { text: "{}".into() });
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(
            wire(&p),
            vec![("A".into(), "1".into()), ("Content-Type".into(), "application/json".into())]
        );
        assert_eq!(p.body.as_deref(), Some("{}"));
    }

    #[test]
    fn explicit_content_type_wins_case_insensitively() {
        let mut r = base("https://x.test");
        r.headers.insert("content-TYPE".into(), on("application/vnd.x+json"));
        r.body = Some(Body::Json { text: "{}".into() });
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(wire(&p), vec![("content-TYPE".into(), "application/vnd.x+json".into())]);
    }

    /// `display_url` masks secret values everywhere they land in the final
    /// URL — path and merged query params alike — while `url` (what's
    /// actually sent) keeps the real values.
    #[test]
    fn display_url_masks_secrets_that_url_keeps() {
        let mut req = base("https://api.example.com/{{tenant}}/things");
        req.params.insert("key".into(), on("{{api_key}}"));
        let mut c = ctx(&[("tenant", "acme"), ("api_key", "s3cret")], &[]);
        c.meta.insert("api_key".into(), varmodel::VarMeta::Secret);
        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(p.url, "https://api.example.com/acme/things?key=s3cret");
        assert_eq!(p.display_url, format!("https://api.example.com/acme/things?key={SECRET_MASK}"));
    }

    /// Each sent row's `display` masks secret values — names untouched,
    /// the auto `Content-Type` included — while `value` keeps the real
    /// text for the wire.
    #[test]
    fn display_headers_mask_secrets_that_headers_keep() {
        let mut req = base("https://api.example.com/things");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("Authorization".into(), on("Bearer {{api_key}}"));
        req.headers.insert("X-Tenant".into(), on("{{tenant}}"));
        let mut c =
            ctx(&[("tenant", "acme"), ("api_key", "s3cret")], &[("X-Trace", "{{api_key}}", true)]);
        c.meta.insert("api_key".into(), varmodel::VarMeta::Secret);
        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![
                ("X-Trace".to_string(), "s3cret".to_string()),
                ("Authorization".to_string(), "Bearer s3cret".to_string()),
                ("X-Tenant".to_string(), "acme".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );
        assert_eq!(
            p.headers.iter().map(|h| (h.name.clone(), h.display.clone())).collect::<Vec<_>>(),
            vec![
                ("X-Trace".to_string(), SECRET_MASK.to_string()),
                ("Authorization".to_string(), format!("Bearer {SECRET_MASK}")),
                ("X-Tenant".to_string(), "acme".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );
    }

    /// With no secrets in play the two URLs are identical.
    #[test]
    fn display_url_equals_url_without_secrets() {
        let req = base("https://api.example.com/{{tenant}}");
        let c = ctx(&[("tenant", "acme")], &[]);
        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(p.display_url, p.url);
    }

    // -----------------------------------------------------------------
    // header_rows (spec stage7 §6; 2026-10-18 auto headers from one source)
    // -----------------------------------------------------------------

    fn row<'a>(rows: &'a [HeaderRow], name: &str) -> &'a HeaderRow {
        rows.iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no {name} row in {rows:#?}"))
    }

    /// The client's own row named `name`.
    fn client<'a>(rows: &'a [HeaderRow], name: &str) -> &'a HeaderRow {
        rows.iter()
            .find(|r| r.origin == HeaderOrigin::Client && r.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no client {name} row in {rows:#?}"))
    }

    #[test]
    fn header_rows_list_an_unsent_default_and_mask_a_secret_in_display_only() {
        let mut req = base("https://api.example.com/v1/users");
        req.headers.insert("X-Auth".into(), on("{{api_key}}"));
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("api_key".into(), "s3cret".into());
        ctx.meta.insert("api_key".into(), varmodel::VarMeta::Secret);
        ctx.default_headers.insert("X-Auth".into(), on("default"));
        let rows = header_rows(&req, &ctx);
        let auth: Vec<_> = rows.iter().filter(|r| r.name.eq_ignore_ascii_case("x-auth")).collect();
        assert!(auth.iter().any(|r| r.origin == HeaderOrigin::Request { row: 0 }
            && r.sent()
            && r.value == "s3cret"
            && r.display == SECRET_MASK));
        assert!(auth.iter().any(|r| r.origin == HeaderOrigin::Default && !r.sent()));
        assert_eq!(client(&rows, "Host").value, "api.example.com");
    }

    #[test]
    fn header_rows_keep_a_sent_default_and_skip_a_disabled_one() {
        let req = base("https://x.test");
        let mut ctx = PrepareContext::default();
        ctx.default_headers.insert("X-Trace".into(), on("on"));
        ctx.default_headers.insert("X-Off".into(), off("nope"));
        let rows = header_rows(&req, &ctx);
        assert_eq!(row(&rows, "X-Trace").origin, HeaderOrigin::Default);
        assert!(row(&rows, "X-Trace").sent());
        assert!(
            !rows.iter().any(|r| r.name == "X-Off"),
            "a disabled default is not part of the send at all"
        );
    }

    #[test]
    fn header_rows_add_auto_content_type_only_with_a_body_and_unsend_it_when_named() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        assert!(
            !header_rows(&req, &PrepareContext::default())
                .iter()
                .any(|r| r.origin == HeaderOrigin::ContentType),
            "no body: no auto content-type"
        );

        req.body = Some(Body::Json { text: "{}".into() });
        let rows = header_rows(&req, &PrepareContext::default());
        let auto = row(&rows, "content-type");
        assert_eq!(auto.origin, HeaderOrigin::ContentType);
        assert_eq!(auto.value, "application/json");
        assert!(auto.sent());

        req.headers.insert("content-TYPE".into(), on("application/vnd.x+json"));
        let rows = header_rows(&req, &PrepareContext::default());
        let auto = rows.iter().find(|r| r.origin == HeaderOrigin::ContentType).unwrap();
        assert!(!auto.sent(), "an explicit content-type wins case-insensitively");
    }

    #[test]
    fn header_rows_leave_unresolved_tokens_literal_and_list_them() {
        let mut req = base("https://x.test");
        req.headers.insert("X-Ghost".into(), on("a{{ghost}}b"));
        let rows = header_rows(&req, &PrepareContext::default());
        let ghost = row(&rows, "X-Ghost");
        assert_eq!(ghost.value, "a{{ghost}}b");
        assert_eq!(ghost.display, "a{{ghost}}b");
        assert_eq!(ghost.unresolved, ["ghost".to_string()]);
    }

    #[test]
    fn header_rows_client_rows_carry_host_and_content_length() {
        let mut req = base("https://{{host}}:8443/v1/users?q=1#frag");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{\"a\": \"{{tok}}\"}".into() });
        req.substitute_body = true;
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("host".into(), "api.example.com".into());
        ctx.vars.insert("tok".into(), "xyz".into());
        let rows = header_rows(&req, &ctx);
        assert_eq!(client(&rows, "Host").value, "api.example.com:8443");
        assert_eq!(client(&rows, "Content-Length").value, "12", "{{\"a\": \"xyz\"}}");
    }

    /// `Host` is what the client sends: lowercased, the scheme's default
    /// port dropped, no userinfo (R4).
    #[test]
    fn header_rows_host_is_the_authority_the_client_sends() {
        for (url, host) in [
            ("https://Example.com:443/p", "example.com"),
            ("http://example.com:80/p", "example.com"),
            ("http://example.com:8080/p", "example.com:8080"),
            ("https://u:pw@example.com/p", "example.com"),
        ] {
            let rows = header_rows(&base(url), &PrepareContext::default());
            assert_eq!(client(&rows, "Host").value, host, "{url}");
            assert_eq!(client(&rows, "Host").display, host, "{url}");
        }
    }

    /// `Host` and `Content-Length` are the client's (R3a): a request or
    /// default row naming one — directly, through a template, in any case
    /// — is listed but not sent, and the client's own row still is.
    #[test]
    fn header_rows_never_send_a_row_naming_a_client_owned_header() {
        let mut req = base("http://example.com/p");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("{{h}}".into(), on("other.test"));
        req.headers.insert("CONTENT-length".into(), on("99"));
        let mut c = ctx(&[("h", "Host")], &[("host", "default.test", true)]);
        c.default_headers.insert("Content-Length".into(), on("7"));
        let rows = header_rows(&req, &c);
        for r in rows.iter().filter(|r| r.origin != HeaderOrigin::Client) {
            if client_owned(&r.name).is_some() {
                assert!(!r.sent(), "{r:?}");
            }
        }
        assert!(rows.iter().any(|r| r.origin == HeaderOrigin::Request { row: 0 } && !r.sent()));
        assert!(rows.iter().any(|r| r.origin == HeaderOrigin::Request { row: 1 } && !r.sent()));
        assert!(client(&rows, "Host").sent());
        assert_eq!(client(&rows, "Host").value, "example.com");
        assert!(client(&rows, "Content-Length").sent());
        assert_eq!(client(&rows, "Content-Length").value, "2");

        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(wire(&p), vec![("Content-Type".into(), "application/json".into())]);
    }

    /// With no body there is no client `Content-Length`, and a typed one is
    /// still never sent.
    #[test]
    fn a_typed_content_length_without_a_body_is_not_sent() {
        let mut req = base("http://example.com/p");
        req.headers.insert("Content-Length".into(), on("5"));
        let rows = header_rows(&req, &PrepareContext::default());
        assert!(!row(&rows, "Content-Length").sent());
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert!(!p.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-length")));
    }

    /// Each unsent row records why, so screens never work it out again —
    /// including whether the client sends its own row for a client-owned
    /// name (not without a body, nor while the host is a `{{token}}`).
    #[test]
    fn unsent_rows_record_why() {
        let mut req = base("http://example.com/p");
        req.headers.insert("Content-Length".into(), on("5"));
        req.headers.insert("Host".into(), on("other.test"));
        req.headers.insert("X-A".into(), on("mine"));
        let mut c = ctx(&[], &[("X-A", "dflt", true), ("X-B", "1", true), ("x-b", "2", true)]);
        let why = |rows: &[HeaderRow], origin: HeaderOrigin| {
            rows.iter().find(|r| r.origin == origin).unwrap().unsent
        };
        let rows = header_rows(&req, &c);
        let owned = |header, client_sends| Some(Unsent::ClientOwned { header, client_sends });
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 0 }), owned("Content-Length", false));
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 1 }), owned("Host", true));
        assert_eq!(rows[0].unsent, Some(Unsent::Replaced), "X-A default, replaced");
        assert_eq!(rows[1].unsent, None, "the first X-B goes out");
        assert_eq!(rows[2].unsent, Some(Unsent::Duplicate), "the second doesn't");

        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.url = "http://{{host}}/p".into();
        c.default_headers.clear();
        let rows = header_rows(&req, &c);
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 0 }), owned("Content-Length", true));
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 1 }), owned("Host", false));
    }

    /// Only what is sent has to resolve (R8): a row that is listed but not
    /// sent never blocks the send.
    #[test]
    fn an_unsent_rows_unresolved_tokens_do_not_block_the_send() {
        let mut req = base("http://example.com/p");
        req.headers.insert("X-{{gone}}".into(), off("x"));
        req.headers.insert("Host".into(), on("{{also_gone}}"));
        let c = ctx(&[], &[("X-{{gone}}", "{{gone_too}}", true)]);
        prepare(&req, &c).expect("neither unsent row has to resolve");

        let c = ctx(&[], &[("X-Sent", "{{gone}}", true)]);
        let PrepareError::Unresolved(causes) = prepare(&req, &c).unwrap_err();
        assert_eq!(causes.into_keys().collect::<Vec<_>>(), ["gone"]);
    }

    /// The client sends `Accept: */*` on its own unless a header names
    /// `accept` — so the auto section says so, in wire order: after the
    /// prepared rows, before `Host`.
    #[test]
    fn header_rows_client_rows_carry_accept_unless_a_sent_header_names_it() {
        let req = base("https://api.example.com/v1");
        let rows = header_rows(&req, &PrepareContext::default());
        let accept = client(&rows, "Accept");
        assert_eq!(accept.value, "*/*");
        assert!(accept.sent());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Accept", "Host"], "wire order");

        let mut req = base("https://api.example.com/v1");
        req.headers.insert("accept".into(), on("text/plain"));
        let rows = header_rows(&req, &PrepareContext::default());
        assert!(
            !client(&rows, "Accept").sent(),
            "a request row named accept replaces the client's"
        );

        // A sent default names it too — and so does the request row that
        // keeps that default from being sent.
        let req = base("https://api.example.com/v1");
        let with_default = ctx(&[], &[("Accept", "text/plain", true)]);
        assert!(!client(&header_rows(&req, &with_default), "Accept").sent());
        let mut req = base("https://api.example.com/v1");
        req.headers.insert("Accept".into(), on("x"));
        assert!(
            !client(&header_rows(&req, &with_default), "Accept").sent(),
            "the request row still names it even though the default is not sent"
        );
    }

    #[test]
    fn header_rows_content_length_ignores_masking_and_follows_the_substitution_flag() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{{api_key}}".into() });
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("api_key".into(), "s3cret".into());
        ctx.meta.insert("api_key".into(), varmodel::VarMeta::Secret);

        let rows = header_rows(&req, &ctx);
        assert_eq!(
            client(&rows, "Content-Length").value,
            "11",
            "flag off: the raw `{{{{api_key}}}}` text is what gets sent"
        );

        req.substitute_body = true;
        let rows = header_rows(&req, &ctx);
        let length = client(&rows, "Content-Length");
        assert_eq!(length.value, "6", "the real body's length");
        assert_eq!(length.display, "6", "the mask must not change the length the client sends");
    }

    #[test]
    fn header_rows_mask_a_secret_inside_the_host() {
        let req = base("https://{{tenant_key}}.api.example.com/v1");
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("tenant_key".into(), "s3cret".into());
        ctx.meta.insert("tenant_key".into(), varmodel::VarMeta::Secret);

        let rows = header_rows(&req, &ctx);
        let host = client(&rows, "Host");
        assert_eq!(host.display, "●●●●●●.api.example.com");
        assert_eq!(host.value, "s3cret.api.example.com");
    }

    /// A masked host goes through the same derivation as the real one: no
    /// userinfo, no default port, lowercased — only the secret is masked.
    #[test]
    fn header_rows_masked_host_follows_the_same_rule_as_the_real_one() {
        let mut c = ctx(&[("pw", "hunter2"), ("tenant", "Acme")], &[]);
        c.meta.insert("pw".into(), varmodel::VarMeta::Secret);
        c.meta.insert("tenant".into(), varmodel::VarMeta::Secret);

        let rows = header_rows(&base("https://user:{{pw}}@Example.com:443/p"), &c);
        let host = client(&rows, "Host");
        assert_eq!((host.value.as_str(), host.display.as_str()), ("example.com", "example.com"));

        let rows = header_rows(&base("https://{{tenant}}.Example.com:443/v1"), &c);
        let host = client(&rows, "Host");
        assert_eq!(host.value, "acme.example.com");
        assert_eq!(host.display, format!("{SECRET_MASK}.example.com"));
    }

    /// A token left in the userinfo doesn't hide a host that is known.
    #[test]
    fn header_rows_keep_host_when_only_the_userinfo_is_unresolved() {
        let rows =
            header_rows(&base("https://{{user}}@api.example.com/p"), &PrepareContext::default());
        assert_eq!(client(&rows, "Host").value, "api.example.com");
    }

    /// Two defaults naming a client-owned header are both dropped by R3a;
    /// neither is chosen over the other, so neither is a duplicate.
    #[test]
    fn two_client_owned_defaults_are_not_reported_as_duplicates() {
        let mut defaults = IndexMap::new();
        defaults.insert("Host".to_string(), on("a.test"));
        defaults.insert("host".to_string(), on("b.test"));
        let c = PrepareContext { default_headers: defaults, ..PrepareContext::default() };
        let (_, warns) = prepare(&base("http://x.test"), &c).unwrap();
        assert!(warns.is_empty(), "{warns:?}");
    }

    #[test]
    fn header_rows_omit_host_when_the_url_is_unresolved() {
        let req = base("https://{{host}}/v1");
        let rows = header_rows(&req, &PrepareContext::default());
        assert!(!rows.iter().any(|r| r.name == "Host"));
        assert!(!rows.iter().any(|r| r.name == "Content-Length"));
    }

    #[test]
    fn header_rows_are_in_send_order_and_skip_disabled_request_rows() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        req.headers.insert("X-Skip".into(), off("2"));
        req.headers.insert("X-A".into(), on("1"));
        req.body = Some(Body::Json { text: "{}".into() });
        let mut ctx = PrepareContext::default();
        ctx.default_headers.insert("X-D".into(), on("d"));
        let rows = header_rows(&req, &ctx);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["X-D", "X-A", "Content-Type", "Accept", "Host", "Content-Length"]
        );
        assert_eq!(
            row(&rows, "X-A").origin,
            HeaderOrigin::Request { row: 1 },
            "the index counts the disabled row before it"
        );
    }

    /// A prepared request splits the sent rows into the ones it sets and
    /// the ones the client adds after them, each in wire order.
    #[test]
    fn prepared_headers_split_the_sent_rows_from_the_clients() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("Accept".into(), on("text/plain"));
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        let names = |rows: &[HeaderRow]| rows.iter().map(|h| h.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&p.headers), ["Accept", "Content-Type"]);
        assert_eq!(names(&p.client_headers), ["Host", "Content-Length"]);
        assert!(p.headers.iter().chain(&p.client_headers).all(|h| h.sent()));
    }

    /// A secret in a header's name is masked on screen like one in its
    /// value, and the real name is what goes on the wire.
    #[test]
    fn a_secret_in_a_header_name_is_masked_in_display_name() {
        let mut c = ctx(&[("key", "s3cret")], &[("X-{{key}}", "1", true)]);
        c.meta.insert("key".into(), varmodel::VarMeta::Secret);
        let rows = header_rows(&base("https://x.test"), &c);
        let r = row(&rows, "X-s3cret");
        assert_eq!(r.display_name, format!("X-{SECRET_MASK}"));
        let (p, _) = prepare(&base("https://x.test"), &c).unwrap();
        assert_eq!(wire(&p), vec![("X-s3cret".to_string(), "1".to_string())]);
    }

    /// The stand-in for the mask only stands in for it: a real host that
    /// holds the same label is sent and shown as it is.
    #[test]
    fn a_real_host_holding_the_mask_stand_in_is_left_alone() {
        let rows =
            header_rows(&base("https://postui-secret-mask.internal/x"), &PrepareContext::default());
        assert_eq!(client(&rows, "Host").value, "postui-secret-mask.internal");
    }
}
