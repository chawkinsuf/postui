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
    /// The headers the request itself sets, in table order: the sent
    /// [`header_rows`] other than the client's. What the request is built
    /// from (and what curl export writes).
    pub headers: Vec<SentHeader>,
    /// Every header the send puts on the wire, in wire order and wire case
    /// (lowercase): `headers` plus what the HTTP client adds on its own (the
    /// URL's `Authorization`, `Host`, `Content-Length`), ordered
    /// by [`wire_order`]. For the screens that show the whole send (the
    /// response pane's `sent` section); never what a request is built from.
    /// `Host` and `Content-Length` are what an HTTP/1.1 send carries; an h2
    /// negotiation spells the first as `:authority`, same value. Verified
    /// against a raw server by the binary's `sent_headers_match_the_wire`.
    pub wire: Vec<SentHeader>,
    pub body: Option<String>,
    /// Skip TLS certificate verification for this send (the request's
    /// `insecure` flag, carried through so the transport can pick a client).
    pub insecure: bool,
}

/// One header that goes on the wire: its real name and value (what the
/// request carries and a copy pill yields) and their masked twins (what
/// screens show while secrets are hidden).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentHeader {
    pub name: String,
    pub display_name: String,
    pub value: String,
    pub display: String,
}

impl SentHeader {
    /// Whether a secret's value is in the name or the value.
    pub fn is_masked(&self) -> bool {
        self.value != self.display || self.name != self.display_name
    }
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
    /// The URL or a query parameter uses `var`, whose value holds
    /// `{{token}}`: a value isn't a template, so the URL would go out with
    /// the braces in it (spec 2026-10-30 D1). Asked before the unresolved
    /// names, so a missing secret or selection (only a warning in `check`)
    /// can't hide it. Never about a secret, whose value isn't looked into.
    ValueHoldsVariable {
        var: String,
        token: String,
    },
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
            PrepareError::ValueHoldsVariable { var, token } => write!(
                f,
                "the URL uses {var}, whose value holds {{{{{token}}}}}: postui doesn't fill in \
                 variables inside a value"
            ),
        }
    }
}

/// D1: the refusal for a URL or query parameter using a value that holds a
/// `{{token}}`, with `vars` overlaid. The URL is the one place checked at
/// send time: a header's or a body's braces might be meant, and `check`
/// warns about every such value where it is written (spec 2026-10-30, the
/// amendment after review round 3). A secret's value is opaque data, not a
/// value someone wrote as a template, so it is never looked into: braces
/// in it go out as they are, and nothing of it is printed.
fn url_holding_token(
    req: &HttpRequest,
    vars: &IndexMap<String, String>,
    secrets: &BTreeSet<String>,
) -> Option<PrepareError> {
    let params = req.params.iter().filter(|(_, e)| e.enabled);
    let mut parts = std::iter::once(req.url.as_str())
        .chain(params.flat_map(|(k, e)| [k.as_str(), e.value.as_str()]));
    parts.find_map(|text| {
        crate::vars::find_tokens(text).into_iter().filter(|t| !secrets.contains(&t.name)).find_map(
            |t| {
                let inner = crate::vars::find_tokens(vars.get(&t.name)?).into_iter().next()?;
                Some(PrepareError::ValueHoldsVariable { var: t.name, token: inner.name })
            },
        )
    })
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
    let secret_names = secret_names(ctx);
    // First, so a missing secret or selection can't hide it from `check`.
    if let Some(e) = url_holding_token(req, &vars, &secret_names) {
        return Err(e);
    }

    let mut missing = BTreeSet::new();
    let no_mask = BTreeSet::new();
    let mut sub = |s: &str| substitute_masked(s, &vars, &no_mask, &mut missing);

    let mut warnings = Vec::new();
    let url = assemble_url(req, &mut sub, &mut warnings);
    // The same assembly again with secret values masked — what screens
    // show for the sent request (`PreparedRequest::display_url`). Its
    // warnings are duplicates of the real pass's, so they're dropped.
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
    let body_len = body.as_ref().map(String::len);
    let rows = rows(req, ctx, &vars, &secret_names, body_len, &mut warnings);
    let wire = wire_order(&rows);
    let headers: Vec<SentHeader> = rows
        .into_iter()
        .filter(|r| r.sent() && r.origin != HeaderOrigin::Client)
        .map(|r| {
            missing.extend(r.unresolved);
            r.header
        })
        .collect();

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
            wire,
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

/// The sent rows of [`header_rows`] in the order they go out, in wire case.
/// reqwest keeps headers in a map that holds all of a name's values
/// together, at the place the name first went in (probed against a raw
/// server, task #22). `rows` lists them in the order they go in: what the
/// client puts in before ours (the URL's `Authorization`), ours, then what
/// it adds after. A row of ours replaces a client row in place, so the one
/// the client puts in before ours keeps its place when replaced. The rows
/// it adds after ours (`Host`, `Content-Length`) are never replaced (R3a),
/// so only rows of ours are ever skipped there.
fn wire_order(rows: &[HeaderRow]) -> Vec<SentHeader> {
    let mut groups: IndexMap<String, Vec<SentHeader>> = IndexMap::new();
    let mut ours_started = false;
    for r in rows {
        ours_started |= r.origin != HeaderOrigin::Client;
        let holds_place = r.origin == HeaderOrigin::Client && !ours_started;
        if !r.sent() && !holds_place {
            continue;
        }
        let name = r.header.name.to_ascii_lowercase();
        let group = groups.entry(name.clone()).or_default();
        if r.sent() {
            let display_name = r.header.display_name.to_ascii_lowercase();
            group.push(SentHeader { name, display_name, ..r.header.clone() });
        }
    }
    groups.into_values().flatten().collect()
}

/// The `Accept` postui sends on every request unless a request row or
/// project default names `accept` (task #174): JSON, because the app is
/// about JSON, and the value curl's `--json` sends. It has no `*/*`
/// fallback on purpose: Rails ignores an Accept with `*/*` next to a comma
/// as a browser's (`BROWSER_LIKE_ACCEPTS`, unless the app opts into
/// `strict_accept_header`) and answers its default format, usually HTML,
/// so a fallback would lose the JSON preference on one of the most common
/// API frameworks. The cost is that a server that only serves another type
/// may answer 406; the pencil or a project default overrides it (the
/// value and that trade are the user's). A curl import with no `Accept`
/// adds no row, so it sends this one too (curl spec, the 2026-10-27
/// amendments under §5.2).
pub const AUTO_ACCEPT: &str = "application/json";

/// Where a header row comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderOrigin {
    /// An enabled row of the request's own table; `row` is its index in
    /// that table (disabled rows counted), so the editor can point at it.
    Request { row: usize },
    /// A project `[default_headers]` row.
    Default,
    /// A row postui adds on its own and puts on the request: the
    /// `Content-Type: application/json` for a body, and the
    /// [`AUTO_ACCEPT`] every send carries (task #174).
    Auto,
    /// Added by the HTTP client itself (the URL's `Authorization`, `Host`,
    /// `Content-Length`): listed so the screens show the whole send, but
    /// never put on the request by us.
    Client,
}

/// One header of a send, as [`header_rows`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderRow {
    /// The header itself: its real name and value (an unresolved
    /// `{{token}}` keeps its braces) and their masked twins — what
    /// [`prepare`] keeps of a row that is sent.
    pub header: SentHeader,
    pub origin: HeaderOrigin,
    /// Why the row doesn't go on the wire, or `None` when it does. A row
    /// that isn't sent is still listed, so screens can say why.
    pub unsent: Option<Unsent>,
    /// Names of `{{tokens}}` in the name or value that didn't resolve.
    pub unresolved: Vec<String>,
    /// How a request row taking this row over is written (task #20): a
    /// default's name and value with their `{{tokens}}`, an auto row's text
    /// as sent. `None` for a row the request may never take over:
    /// its own rows, which the table already holds; a header the client
    /// owns (R3a); the rows the client reads off the URL (`Host`, the
    /// userinfo's `Authorization`, which copied into the request would
    /// write the URL's credentials into the request file in the clear);
    /// and any row the request has a row naming, enabled or disabled,
    /// which is the one to edit. Whether the row is still sent is
    /// [`HeaderRow::overriding_row`]'s to add.
    ///
    /// It carries the text, not a flag, so the click reads it off the one
    /// list rather than finding the row's default again by name. The cost
    /// is a copy of each default's name and value, and of `Accept`'s and
    /// `Content-Type`'s, per call: a few short strings per frame of the
    /// Headers tab and per send, beside the substitution and masking that
    /// already allocate for every row.
    pub override_as: Option<(String, String)>,
}

impl HeaderRow {
    /// Whether the row goes on the wire.
    pub fn sent(&self) -> bool {
        self.unsent.is_none()
    }

    /// The name and value of a request row that would take this row over,
    /// so the user can give it a value of their own (task #20): its
    /// [`HeaderRow::override_as`], while the row is sent. A row something
    /// already overrides has none.
    pub fn overriding_row(&self) -> Option<(&str, &str)> {
        self.override_as.as_ref().filter(|_| self.sent()).map(|(n, v)| (n.as_str(), v.as_str()))
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
    /// It names a [`CLIENT_OWNED`] header (R3a), in its usual spelling.
    ClientOwned { header: &'static str },
}

/// Headers the HTTP client sets from the URL and the body, and which
/// nothing may override (spec 2026-10-18 R3a): a request or default row
/// with one of these names is listed but never sent, and the client's own
/// row always is. `Transfer-Encoding` frames the body: the client never
/// sends one for a body of known length, and hyper rewrites a typed one
/// (adds `chunked`, drops it with no body) and drops `Content-Length` for
/// it — so a typed one is the client's to refuse too (task #22).
const CLIENT_OWNED: [&str; 3] = ["Host", "Content-Length", "Transfer-Encoding"];

/// The [`CLIENT_OWNED`] header `name` names, in its usual spelling. Kept
/// private: callers read the verdict off [`Unsent::ClientOwned`] instead of
/// asking about names again.
fn client_owned(name: &str) -> Option<&'static str> {
    CLIENT_OWNED.into_iter().find(|owned| name.eq_ignore_ascii_case(owned))
}

/// A row [`header_rows`] adds of its own rather than reads off the request
/// or the defaults, with no `{{tokens}}` to resolve: postui's auto rows and
/// the client's. The caller says whose it is and whether the request may
/// take it over, so neither kind is built as the other and patched.
fn fixed_row(
    name: &str,
    value: String,
    display: String,
    origin: HeaderOrigin,
    unsent: Option<Unsent>,
    override_as: Option<(String, String)>,
) -> HeaderRow {
    HeaderRow {
        header: SentHeader { name: name.into(), display_name: name.into(), display, value },
        origin,
        unsent,
        unresolved: Vec::new(),
        override_as,
    }
}

/// Every header of `req`'s send, in wire order — the client's
/// `Authorization` from the URL's userinfo, enabled defaults, enabled
/// request rows, the auto `Content-Type` and `Accept`, then the client's
/// `Host` and `Content-Length` — whether sent or not. The order is the
/// order rows go into the client's header map, which [`wire_order`] reads
/// to place the client's rows (one put in before ours keeps its place
/// when replaced), so it is a contract, not a display choice. The one
/// place a send's headers are worked out: [`prepare`] keeps the sent
/// rows, and the editor's auto section draws the rest of the story.
/// Never errors: unresolved tokens stay literal and are listed per row.
pub fn header_rows(req: &HttpRequest, ctx: &PrepareContext) -> Vec<HeaderRow> {
    let vars = overlaid_vars(req, ctx);
    let body_len = sent_body(req).map(|body| match req.substitute_body {
        true => substituted_len(body, &vars),
        false => body.len(),
    });
    rows(req, ctx, &vars, &secret_names(ctx), body_len, &mut Vec::new())
}

/// The byte length `text` has once [`crate::vars::substitute`] has run on
/// it (an unresolved token stays as written), counted without building the
/// string: the Headers tab asks for a body's on every frame.
fn substituted_len(text: &str, vars: &IndexMap<String, String>) -> usize {
    crate::vars::find_tokens(text).iter().fold(text.len(), |len, t| match vars.get(&t.name) {
        Some(value) => len - (t.end - t.start) + value.len(),
        None => len,
    })
}

/// The rows of [`header_rows`], from what the caller already worked out:
/// the overlaid `vars`, the `secrets` to mask, and the byte length of the
/// body the send carries — so [`prepare`] substitutes nothing twice.
/// Duplicate defaults are reported to `warnings`.
fn rows(
    req: &HttpRequest,
    ctx: &PrepareContext,
    vars: &IndexMap<String, String>,
    secrets: &BTreeSet<String>,
    body_len: Option<usize>,
    warnings: &mut Vec<PrepareWarning>,
) -> Vec<HeaderRow> {
    let no_mask = BTreeSet::new();
    let quiet = |s: &str| substitute_masked(s, vars, &no_mask, &mut BTreeSet::new());
    // The screens' version of `text`: the real one when no secret is in
    // play, which is most projects, on every frame the Headers tab shows.
    let masked = |text: &str, real: &String| {
        if secrets.is_empty() {
            real.clone()
        } else {
            substitute_masked(text, vars, secrets, &mut BTreeSet::new())
        }
    };
    // Whether the request has a row, enabled or disabled, naming `name`
    // (case-insensitively, after substitution: `{{h}}` with h = Accept is
    // an Accept row). Such a row is the one to edit, so no other row of
    // that name may be taken over (`override_as`).
    let request_names: Vec<String> = req.headers.keys().map(|k| quiet(k)).collect();
    let named_by_request = |name: &str| request_names.iter().any(|n| n.eq_ignore_ascii_case(name));
    // Substitutes name and value together, so a row reports the
    // unresolved tokens of both halves.
    let row = |name: &str, value: &str, origin: HeaderOrigin| {
        let mut unresolved = BTreeSet::new();
        let real_name = substitute_masked(name, vars, &no_mask, &mut unresolved);
        let real = substitute_masked(value, vars, &no_mask, &mut unresolved);
        let owned = client_owned(&real_name);
        let may_override =
            origin == HeaderOrigin::Default && owned.is_none() && !named_by_request(&real_name);
        HeaderRow {
            unsent: owned.map(|header| Unsent::ClientOwned { header }),
            header: SentHeader {
                display_name: masked(name, &real_name),
                name: real_name,
                display: masked(value, &real),
                value: real,
            },
            override_as: may_override.then(|| (name.into(), value.into())),
            origin,
            unresolved: unresolved.into_iter().collect(),
        }
    };
    // A row the client adds, shown as `display`: masked when a secret can
    // change it (the URL's rows), else as it is sent. The request may not
    // take it over.
    let client_row = |name: &str, value: String, display: String, unsent: Option<Unsent>| {
        fixed_row(name, value, display, HeaderOrigin::Client, unsent, None)
    };
    let mut rows = Vec::new();

    // Defaults merge UNDER the request's rows: a default is not sent when
    // the request has any row, enabled or disabled, whose name matches
    // case-insensitively after substitution (`{{h}}` with h = Accept is an
    // Accept row). Of two defaults resolving to the same lowercase name,
    // the first is sent and the rest are reported — unless the name is
    // client-owned, when none of them is sent and there is no choice to
    // report.
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for (k, e) in ctx.default_headers.iter().filter(|(_, e)| e.enabled) {
        let mut r = row(k, &e.value, HeaderOrigin::Default);
        let lower = r.header.name.to_ascii_lowercase();
        // A client-owned default is already unsent, whatever else names it.
        if r.sent() && named_by_request(&r.header.name) {
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

    // An auto or client row stands aside for a sent row naming it. A
    // disabled request row is not one: it leaves the auto `Content-Type` or
    // `Accept` sent, as an untyped request would, and it is the row to edit
    // (so the auto row offers no pencil; auto-headers spec, task #20). To
    // ask for something other than JSON, the row is enabled with that
    // value, `*/*` included (task #174: "the user can override it").
    let replaced = |rows: &[HeaderRow], name: &str| {
        rows.iter()
            .any(|r| r.sent() && r.header.name.eq_ignore_ascii_case(name))
            .then_some(Unsent::Replaced)
    };
    // postui's own rows, put on the request like a typed one: a body's
    // `Content-Type`, then [`AUTO_ACCEPT`] on every send (task #174).
    // Left to the client, `Accept` would be reqwest's `*/*`, and Copy as
    // curl would leave it to curl's, so it is ours to send. A request row
    // or project default naming `accept` (the pencil writes one) replaces
    // it. Each is shown as it is sent, and the request may take it over
    // unless it has a row naming it already.
    let auto = |name: &str, value: &str, rows: &[HeaderRow]| {
        let take_over = (!named_by_request(name)).then(|| (name.into(), value.into()));
        let (unsent, origin) = (replaced(rows, name), HeaderOrigin::Auto);
        fixed_row(name, value.into(), value.into(), origin, unsent, take_over)
    };
    if body_len.is_some() {
        rows.push(auto("Content-Type", "application/json", &rows));
    }
    rows.push(auto("Accept", AUTO_ACCEPT, &rows));

    // The client's own rows. `Host` and the URL's `Authorization` are read
    // off the substituted template rather than `prepare`'s assembled URL so
    // `header_rows` needs no assembly: the two share an authority, since
    // merging `[params]` only rewrites what follows `?` and `#`.
    let mut unresolved = BTreeSet::new();
    let real = url_headers(&substitute_masked(&req.url, vars, &no_mask, &mut unresolved));
    let url_tokens = crate::vars::find_tokens(&req.url);
    // Whether the values behind `stand_ins` can change the header `pick`
    // reads: the two stand-ins disagree, or differ from the real one.
    // Fail-closed: a stand-in URL the parser refuses (a secret scheme, say)
    // counts as changed.
    let changed = |stand_ins: &Option<[UrlHeaders; 2]>,
                   pick: fn(&UrlHeaders) -> &Option<String>| {
        stand_ins
            .as_ref()
            .is_some_and(|[one, two]| pick(one) != pick(two) || pick(one) != pick(&real))
    };
    // Shown exactly as sent unless a secret can change it; then masked,
    // since no account of which part of it is the secret's can be trusted
    // to be the parser's.
    let by_secret = with_stand_ins(&req.url, &url_tokens, vars, secrets);
    // reqwest turns the URL's userinfo into `Authorization: Basic …` as it
    // builds the request, before any row of ours, so it leads the wire. A
    // sent row naming `authorization` replaces it in place (`wire_order`).
    // None while a token still unresolved can change it.
    let authorization: fn(&UrlHeaders) -> &Option<String> = |h| &h.authorization;
    let by_token = || with_stand_ins(&req.url, &url_tokens, vars, &unresolved);
    if let Some(auth) = real.authorization.clone().filter(|_| !changed(&by_token(), authorization))
    {
        let display = if changed(&by_secret, authorization) {
            format!("Basic {SECRET_MASK}")
        } else {
            auth.clone()
        };
        rows.insert(
            0,
            client_row("Authorization", auth, display, replaced(&rows, "authorization")),
        );
    }
    // `Host` is the URL's authority as the client derives it, real and
    // masked. A URL whose host is still a `{{token}}` has none yet.
    if let Some(host) = real.host.clone() {
        let display =
            if changed(&by_secret, |h| &h.host) { SECRET_MASK.to_string() } else { host.clone() };
        rows.push(client_row("Host", host, display, None));
    }
    // The byte length of what goes on the wire, never the masked body. An
    // empty body goes out with none (hyper sends no `content-length: 0`).
    if let Some(len) = body_len.filter(|&len| len > 0) {
        // The client's, like `Host`: never taken over (R3a).
        rows.push(client_row("Content-Length", len.to_string(), len.to_string(), None));
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

/// The headers the client derives from a URL on its own, read with one
/// parse.
#[derive(Debug, PartialEq, Eq)]
struct UrlHeaders {
    host: Option<String>,
    authorization: Option<String>,
}

fn url_headers(url: &str) -> UrlHeaders {
    let parsed = url::Url::parse(url).ok();
    UrlHeaders {
        authorization: parsed.as_ref().and_then(userinfo_auth),
        host: authority(url, parsed.as_ref()),
    }
}

/// [`url_headers`] of `template` (whose `tokens` the caller has found) with
/// every name in `names` given the value `1`, then `2`; `None` when none of
/// them is written in it. Two different
/// answers for a header mean those names' values can change it. Asked of
/// the parser, never answered by string rules about where a URL's parts sit
/// (every such rule missed one of the parser's cases: `https:/host`,
/// `127.1`, a `/` inside a password moving the split). `1` and `2` fit
/// wherever a value can sit — host label, port, userinfo, path — so a value
/// that reaches a header gives two different ones.
fn with_stand_ins(
    template: &str,
    tokens: &[crate::vars::TokenSpan],
    vars: &IndexMap<String, String>,
    names: &BTreeSet<String>,
) -> Option<[UrlHeaders; 2]> {
    if !tokens.iter().any(|t| names.contains(&t.name)) {
        return None;
    }
    // Only the template's own tokens, not the whole of `vars`: the editor
    // asks on every frame.
    let derive = |stand_in: &str| {
        let values: IndexMap<String, String> = tokens
            .iter()
            .filter_map(|t| {
                let value = if names.contains(&t.name) {
                    Some(stand_in.to_string())
                } else {
                    vars.get(&t.name).cloned()
                };
                value.map(|v| (t.name.clone(), v))
            })
            .collect();
        url_headers(&crate::vars::substitute(template, &values, &mut BTreeSet::new()))
    };
    Some([derive("1"), derive("2")])
}

/// The `Basic` credentials value for `user:password`: what reqwest sends
/// for a URL's userinfo and curl for `-u`.
pub(crate) fn basic_auth(credentials: &str) -> String {
    use base64::Engine as _;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(credentials))
}

/// The `Authorization` reqwest derives from a URL's userinfo, each part
/// percent-decoded. A copy of reqwest's `extract_authority`, so the
/// binary's `sent_headers_match_the_wire` pins it against the wire: none
/// without userinfo or when the username isn't UTF-8 once decoded, and a
/// password that isn't is left off.
fn userinfo_auth(u: &url::Url) -> Option<String> {
    let decode = |part: &str| {
        percent_encoding::percent_decode_str(part).decode_utf8().ok().map(String::from)
    };
    let user = decode(u.username())?;
    let password = u.password().and_then(decode);
    if user.is_empty() && password.is_none() {
        return None;
    }
    Some(basic_auth(&format!("{user}:{}", password.unwrap_or_default())))
}

/// The `Host` value the client derives from `url` (`parsed`, if the parser
/// takes it): the host, lowercased (and punycoded), plus the port when it
/// isn't the scheme's default — no userinfo. `None` while the host is empty
/// or still a `{{token}}`. A URL the parser refuses keeps its host and port
/// as written (the send itself will report that URL).
fn authority(url: &str, parsed: Option<&url::Url>) -> Option<String> {
    if let Some(u) = parsed
        && let Some(host) = u.host_str()
    {
        // The parser takes braces in a host as they are (or escaped), so
        // a host that is still a token is ruled out here too.
        let lower = host.to_ascii_lowercase();
        if host.is_empty() || host.contains('{') || lower.contains("%7b") {
            return None;
        }
        return Some(match u.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        });
    }
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let segment = after_scheme.split(['/', '?', '#']).next().unwrap_or_default();
    let written = segment.rsplit_once('@').map_or(segment, |(_, host_port)| host_port);
    (!written.is_empty() && !written.contains("{{")).then(|| written.to_string())
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

    /// The auto `Accept` every send carries after the rows written for it,
    /// unless one of them names `accept` (task #174). Spelled out rather
    /// than read from [`AUTO_ACCEPT`], here and in the app's tests: they pin
    /// what the user sees on the wire and on screen, so a change to the
    /// constant has to change them on purpose.
    fn json_accept() -> (String, String) {
        ("Accept".into(), "application/json".into())
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
                !rows
                    .iter()
                    .any(|r| r.header.name == "Content-Type" || r.header.name == "Content-Length"),
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
        let causes = unresolved(prepare(&r, &PrepareContext::default()));
        assert_eq!(
            causes.into_keys().collect::<Vec<_>>(),
            vec!["gone".to_string()],
            "body ignored while flag off"
        );
        r.substitute_body = true;
        let causes = unresolved(prepare(&r, &PrepareContext::default()));
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
            vec![("x-api".to_string(), "literal".to_string()), json_accept()],
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
            vec![("Authorization".to_string(), "Bearer req".to_string()), json_accept()],
            "the substituted request name suppresses the default"
        );
    }

    #[test]
    fn suppression_probe_does_not_report_a_disabled_rows_unresolved_name() {
        let mut r = base("http://x.test");
        r.headers.insert("{{nope}}".into(), off("ignored"));
        let c = ctx(&[], &[("X-Default", "d", true)]);
        let (p, _) = prepare(&r, &c).expect("a disabled row's template is never unresolved");
        assert_eq!(wire(&p), vec![("X-Default".to_string(), "d".to_string()), json_accept()]);
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
        assert_eq!(
            wire(&p),
            vec![json_accept()],
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
            vec![("X-One".to_string(), "first".to_string()), json_accept()],
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
        let causes = unresolved(prepare(&r, &PrepareContext::default()));
        assert_eq!(causes.get("name"), Some(&UnresolvedCause::Undefined));
    }

    #[test]
    fn unresolved_causes_mapped_from_context_meta() {
        let r = base("http://x.test/{{gone}}/{{user}}/{{api_key}}");
        let mut c = PrepareContext::default();
        c.meta.insert("user".into(), varmodel::VarMeta::NeedsSelection);
        c.meta.insert("api_key".into(), varmodel::VarMeta::MissingSecret);
        let causes = unresolved(prepare(&r, &c));
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

    /// The causes of an `Unresolved` refusal; any other outcome fails the
    /// test.
    fn unresolved(
        outcome: Result<(PreparedRequest, Vec<PrepareWarning>), PrepareError>,
    ) -> BTreeMap<String, UnresolvedCause> {
        match outcome {
            Err(PrepareError::Unresolved(causes)) => causes,
            other => panic!("expected an unresolved refusal, got {other:?}"),
        }
    }

    fn holds(var: &str, token: &str) -> PrepareError {
        PrepareError::ValueHoldsVariable { var: var.into(), token: token.into() }
    }

    /// D1 (spec 2026-10-30): a value isn't a template, so a URL token whose
    /// value holds a `{{token}}` would go out with its braces; the send is
    /// refused instead, naming both.
    #[test]
    fn a_url_token_whose_value_holds_a_token_refuses_the_send() {
        let r = base("{{base}}/api");
        let c = ctx(&[("base", "https://{{host}}"), ("host", "x.test")], &[]);
        assert_eq!(prepare(&r, &c).unwrap_err(), holds("base", "host"));
        assert_eq!(
            holds("base", "host").to_string(),
            "the URL uses base, whose value holds {{host}}: postui doesn't fill in variables \
             inside a value"
        );
    }

    /// D1: the query parameters are part of the URL, key and value alike;
    /// a disabled one isn't sent, so it doesn't count.
    #[test]
    fn a_query_parameter_whose_value_holds_a_token_refuses_the_send() {
        let c = ctx(&[("k", "{{a}}"), ("v", "x{{b}}")], &[]);
        let mut r = base("http://x.test");
        r.params.insert("{{k}}".into(), on("1"));
        assert_eq!(prepare(&r, &c).unwrap_err(), holds("k", "a"));
        let mut r = base("http://x.test");
        r.params.insert("id".into(), on("{{v}}"));
        assert_eq!(prepare(&r, &c).unwrap_err(), holds("v", "b"));
        let mut r = base("http://x.test");
        r.params.insert("id".into(), off("{{v}}"));
        assert!(prepare(&r, &c).is_ok(), "a disabled parameter isn't sent");
    }

    /// D1 (amended): the URL's refusal comes before an unresolved name, so a
    /// missing secret (only a warning in `check`) can't hide it.
    #[test]
    fn a_value_holding_a_token_is_reported_before_an_unresolved_name() {
        let r = base("{{base}}/{{api_key}}");
        let mut c = ctx(&[("base", "https://{{host}}")], &[]);
        c.meta.insert("api_key".into(), varmodel::VarMeta::MissingSecret);
        assert_eq!(prepare(&r, &c).unwrap_err(), holds("base", "host"));
    }

    /// D1: a secret's value is opaque data, never looked into: braces in it
    /// go out as they are, and nothing of it is printed.
    #[test]
    fn a_secrets_value_is_never_looked_into() {
        let r = base("http://x.test/{{key}}");
        let mut c = ctx(&[("key", "ab{{cdEF12}}gh")], &[]);
        c.meta.insert("key".into(), varmodel::VarMeta::Secret);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.url, "http://x.test/ab{{cdEF12}}gh");
    }

    /// D2 (amended): a header or body using such a value is sent as
    /// written, with no warning at send time: `check` warns where the value
    /// is written.
    #[test]
    fn a_header_or_body_value_holding_a_token_is_sent_as_written() {
        let c = ctx(&[("tok", "{{x}}"), ("doc", "{{z}}")], &[]);
        let mut r = base("http://x.test");
        r.method = Method::Post;
        r.headers.insert("Authorization".into(), on("Bearer {{tok}}"));
        r.body = Some(Body::Json { text: "{{doc}}".into() });
        r.substitute_body = true;
        let (p, warnings) = prepare(&r, &c).unwrap();
        assert!(wire(&p).contains(&("Authorization".into(), "Bearer {{x}}".into())));
        assert_eq!(p.body.as_deref(), Some("{{z}}"));
        assert_eq!(warnings, vec![]);
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
            jq_views: Vec::new(),
            jq_current: None,
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
            vec![
                ("A".into(), "1".into()),
                ("Content-Type".into(), "application/json".into()),
                json_accept()
            ]
        );
        assert_eq!(p.body.as_deref(), Some("{}"));
    }

    #[test]
    fn explicit_content_type_wins_case_insensitively() {
        let mut r = base("https://x.test");
        r.headers.insert("content-TYPE".into(), on("application/vnd.x+json"));
        r.body = Some(Body::Json { text: "{}".into() });
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(
            wire(&p),
            vec![("content-TYPE".into(), "application/vnd.x+json".into()), json_accept()]
        );
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
                json_accept(),
            ]
        );
        assert_eq!(
            p.headers.iter().map(|h| (h.name.clone(), h.display.clone())).collect::<Vec<_>>(),
            vec![
                ("X-Trace".to_string(), SECRET_MASK.to_string()),
                ("Authorization".to_string(), format!("Bearer {SECRET_MASK}")),
                ("X-Tenant".to_string(), "acme".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
                json_accept(),
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
            .find(|r| r.header.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no {name} row in {rows:#?}"))
    }

    /// The client's own row named `name`.
    fn client<'a>(rows: &'a [HeaderRow], name: &str) -> &'a HeaderRow {
        rows.iter()
            .find(|r| r.origin == HeaderOrigin::Client && r.header.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no client {name} row in {rows:#?}"))
    }

    /// postui's own row named `name`, if it adds one.
    fn auto<'a>(rows: &'a [HeaderRow], name: &str) -> Option<&'a HeaderRow> {
        rows.iter().find(|r| r.origin == HeaderOrigin::Auto && r.header.name == name)
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
        let auth: Vec<_> =
            rows.iter().filter(|r| r.header.name.eq_ignore_ascii_case("x-auth")).collect();
        assert!(auth.iter().any(|r| r.origin == HeaderOrigin::Request { row: 0 }
            && r.sent()
            && r.header.value == "s3cret"
            && r.header.display == SECRET_MASK));
        assert!(auth.iter().any(|r| r.origin == HeaderOrigin::Default && !r.sent()));
        assert_eq!(client(&rows, "Host").header.value, "api.example.com");
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
            !rows.iter().any(|r| r.header.name == "X-Off"),
            "a disabled default is not part of the send at all"
        );
    }

    #[test]
    fn header_rows_add_auto_content_type_only_with_a_body_and_unsend_it_when_named() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        assert_eq!(
            auto(&header_rows(&req, &PrepareContext::default()), "Content-Type"),
            None,
            "no body: no auto content-type"
        );

        req.body = Some(Body::Json { text: "{}".into() });
        let rows = header_rows(&req, &PrepareContext::default());
        let ct = row(&rows, "content-type");
        assert_eq!(ct.origin, HeaderOrigin::Auto);
        assert_eq!(ct.header.value, "application/json");
        assert!(ct.sent());

        req.headers.insert("content-TYPE".into(), on("application/vnd.x+json"));
        let rows = header_rows(&req, &PrepareContext::default());
        let ct = auto(&rows, "Content-Type").unwrap();
        assert!(!ct.sent(), "an explicit content-type wins case-insensitively");
    }

    #[test]
    fn header_rows_leave_unresolved_tokens_literal_and_list_them() {
        let mut req = base("https://x.test");
        req.headers.insert("X-Ghost".into(), on("a{{ghost}}b"));
        let rows = header_rows(&req, &PrepareContext::default());
        let ghost = row(&rows, "X-Ghost");
        assert_eq!(ghost.header.value, "a{{ghost}}b");
        assert_eq!(ghost.header.display, "a{{ghost}}b");
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
        assert_eq!(client(&rows, "Host").header.value, "api.example.com:8443");
        assert_eq!(client(&rows, "Content-Length").header.value, "12", "{{\"a\": \"xyz\"}}");
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
            assert_eq!(client(&rows, "Host").header.value, host, "{url}");
            assert_eq!(client(&rows, "Host").header.display, host, "{url}");
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
            if client_owned(&r.header.name).is_some() {
                assert!(!r.sent(), "{r:?}");
            }
        }
        assert!(rows.iter().any(|r| r.origin == HeaderOrigin::Request { row: 0 } && !r.sent()));
        assert!(rows.iter().any(|r| r.origin == HeaderOrigin::Request { row: 1 } && !r.sent()));
        assert!(client(&rows, "Host").sent());
        assert_eq!(client(&rows, "Host").header.value, "example.com");
        assert!(client(&rows, "Content-Length").sent());
        assert_eq!(client(&rows, "Content-Length").header.value, "2");

        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(
            wire(&p),
            vec![("Content-Type".into(), "application/json".into()), json_accept()]
        );
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

    /// Each unsent row records why, so screens never work it out again. A
    /// client-owned name is the client's whether or not the client sends
    /// its own row (not without a body, nor while the host is a
    /// `{{token}}`).
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
        let owned = |header| Some(Unsent::ClientOwned { header });
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 0 }), owned("Content-Length"));
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 1 }), owned("Host"));
        assert_eq!(rows[0].unsent, Some(Unsent::Replaced), "X-A default, replaced");
        assert_eq!(rows[1].unsent, None, "the first X-B goes out");
        assert_eq!(rows[2].unsent, Some(Unsent::Duplicate), "the second doesn't");

        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.url = "http://{{host}}/p".into();
        c.default_headers.clear();
        let rows = header_rows(&req, &c);
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 0 }), owned("Content-Length"));
        assert_eq!(why(&rows, HeaderOrigin::Request { row: 1 }), owned("Host"));
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
        let causes = unresolved(prepare(&req, &c));
        assert_eq!(causes.into_keys().collect::<Vec<_>>(), ["gone"]);
    }

    /// The auto `Accept: application/json` row, for any method.
    fn auto_accept(rows: &[HeaderRow]) -> &HeaderRow {
        auto(rows, "Accept").unwrap_or_else(|| panic!("no auto Accept row in {rows:#?}"))
    }

    /// postui asks for JSON on every send, a GET included (task #174),
    /// unless a header names `accept` — and puts the row on the request
    /// itself, so the send and Copy as curl carry it. Wire order: after the
    /// prepared rows, before `Host`.
    #[test]
    fn header_rows_carry_an_auto_json_accept_unless_a_sent_header_names_it() {
        let req = base("https://api.example.com/v1");
        let rows = header_rows(&req, &PrepareContext::default());
        let accept = auto_accept(&rows);
        assert_eq!(accept.header.value, "application/json");
        assert!(accept.sent());
        let names: Vec<&str> = rows.iter().map(|r| r.header.name.as_str()).collect();
        assert_eq!(names, ["Accept", "Host"], "wire order");
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(wire(&p), vec![json_accept()], "a GET sends it");

        let mut post = base("https://api.example.com/v1");
        post.method = Method::Post;
        post.body = Some(Body::Json { text: "{}".into() });
        let names: Vec<String> = header_rows(&post, &PrepareContext::default())
            .iter()
            .map(|r| r.header.name.clone())
            .collect();
        assert_eq!(names, ["Content-Type", "Accept", "Host", "Content-Length"], "wire order");

        let mut req = base("https://api.example.com/v1");
        req.headers.insert("accept".into(), on("text/plain"));
        let rows = header_rows(&req, &PrepareContext::default());
        assert!(!auto_accept(&rows).sent(), "a request row named accept replaces the auto one");
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(wire(&p), vec![("accept".into(), "text/plain".into())], "and alone goes out");

        // A sent default names it too — and so does the request row that
        // keeps that default from being sent.
        let req = base("https://api.example.com/v1");
        let with_default = ctx(&[], &[("Accept", "text/plain", true)]);
        assert!(!auto_accept(&header_rows(&req, &with_default)).sent());
        let mut req = base("https://api.example.com/v1");
        req.headers.insert("Accept".into(), on("x"));
        assert!(
            !auto_accept(&header_rows(&req, &with_default)).sent(),
            "the request row still names it even though the default is not sent"
        );
    }

    /// The length is counted without building the string, and agrees with
    /// what the substitution builds — unresolved and multibyte tokens too.
    #[test]
    fn substituted_len_matches_the_substituted_text() {
        let vars: IndexMap<String, String> =
            [("a", "ünïcode"), ("b", ""), ("c", "x")].map(|(k, v)| (k.into(), v.into())).into();
        for text in ["", "plain", "{{a}}", "x{{a}}y{{b}}z{{gone}}", "{{c}}{{c}}{{ a }}", "{{"] {
            let built = crate::vars::substitute(text, &vars, &mut BTreeSet::new());
            assert_eq!(substituted_len(text, &vars), built.len(), "{text:?}");
        }
    }

    /// The Host shows exactly as sent unless a secret can change it, then
    /// as the bare mask. "Can change it" is the parser's answer, not a
    /// string rule's: a secret only in the path leaves odd URL shapes, IP
    /// shorthands, IDN hosts and padded ports as the client sends them,
    /// and draws no reveal toggle for them.
    #[test]
    fn the_host_shows_as_sent_unless_a_secret_can_change_it() {
        let mut c = ctx(&[("t", "acme"), ("port", "8443"), ("k", "s3cret")], &[]);
        for name in ["t", "port", "k"] {
            c.meta.insert(name.into(), varmodel::VarMeta::Secret);
        }
        let host = |url: &str| client(&header_rows(&base(url), &c), "Host").header.clone();
        for (url, sent) in [
            ("https:/example.com/v1?k={{k}}", "example.com"),
            ("https://example.com:0443/{{k}}", "example.com"),
            ("http://127.1/{{k}}", "127.0.0.1"),
            ("https://Bücher.example/p?key={{k}}", "xn--bcher-kva.example"),
            ("http://example.com\\{{k}}", "example.com"),
            ("https://user:{{k}}@example.com/p", "example.com"),
        ] {
            let h = host(url);
            assert_eq!((h.value.as_str(), h.display.as_str()), (sent, sent), "{url}");
            assert!(!h.is_masked(), "{url}: nothing to reveal");
        }
        for url in ["https://{{t}}café.com/p", "https://Api.example.com:{{port}}/p"] {
            let h = host(url);
            assert_eq!(h.display, SECRET_MASK, "{url}");
            assert!(!h.display.contains("acme") && !h.display.contains("8443"));
        }
    }

    /// The parser takes `https:/host` and `https:///host` for `https://host`
    /// too: a secret written in such a host never shows through the Host.
    #[test]
    fn a_secret_in_an_odd_slash_url_never_shows_through_the_host() {
        let mut c = ctx(&[("tenant", "s3cret")], &[]);
        c.meta.insert("tenant".into(), varmodel::VarMeta::Secret);
        for url in ["https:///{{tenant}}.api.example.com/p", "https:/{{tenant}}.api.example.com/p"]
        {
            let rows = header_rows(&base(url), &c);
            for host in rows.iter().filter(|r| r.header.name == "Host") {
                assert!(!host.header.display.contains("s3cret"), "{url}: {host:?}");
            }
        }
        let rows = header_rows(&base("https:/{{tenant}}.api.example.com/p"), &c);
        assert_eq!(
            client(&rows, "Host").header.value,
            "s3cret.api.example.com",
            "the parser's host"
        );
    }

    /// A secret written anywhere a value can reach the Host — scheme,
    /// userinfo, host, port — keeps the real Host off the screen, even
    /// when its value moves the split (a `/` in a password makes the
    /// client take `user:123` for the host and port).
    #[test]
    fn a_secret_that_moves_the_hosts_split_never_shows_through_it() {
        for pw in ["123/x", "a#b", "q?r"] {
            let mut c = ctx(&[("pw", pw)], &[]);
            c.meta.insert("pw".into(), varmodel::VarMeta::Secret);
            let rows = header_rows(&base("https://user:{{pw}}@example.com/p"), &c);
            let host = client(&rows, "Host");
            assert_ne!(host.header.value, "example.com", "{pw}: the split really did move");
            assert_eq!(host.header.display, SECRET_MASK, "{pw}: the secret changed the Host");
        }
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
            client(&rows, "Content-Length").header.value,
            "11",
            "flag off: the raw `{{{{api_key}}}}` text is what gets sent"
        );

        req.substitute_body = true;
        let rows = header_rows(&req, &ctx);
        let length = client(&rows, "Content-Length");
        assert_eq!(length.header.value, "6", "the real body's length");
        assert_eq!(
            length.header.display, "6",
            "the mask must not change the length the client sends"
        );
    }

    #[test]
    fn header_rows_mask_a_secret_inside_the_host() {
        let req = base("https://{{tenant_key}}.api.example.com/v1");
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("tenant_key".into(), "s3cret".into());
        ctx.meta.insert("tenant_key".into(), varmodel::VarMeta::Secret);

        let rows = header_rows(&req, &ctx);
        let host = client(&rows, "Host");
        assert_eq!(
            host.header.display, SECRET_MASK,
            "the secret shapes the Host: all of it masked"
        );
        assert_eq!(host.header.value, "s3cret.api.example.com");
    }

    /// A secret in the userinfo leaves the Host as sent (no userinfo, no
    /// default port, lowercased); one in a host label masks all of it.
    #[test]
    fn header_rows_masked_host_follows_the_same_rule_as_the_real_one() {
        let mut c = ctx(&[("pw", "hunter2"), ("tenant", "Acme")], &[]);
        c.meta.insert("pw".into(), varmodel::VarMeta::Secret);
        c.meta.insert("tenant".into(), varmodel::VarMeta::Secret);

        let rows = header_rows(&base("https://user:{{pw}}@Example.com:443/p"), &c);
        let host = client(&rows, "Host");
        assert_eq!(
            (host.header.value.as_str(), host.header.display.as_str()),
            ("example.com", "example.com")
        );

        let rows = header_rows(&base("https://{{tenant}}.Example.com:443/v1"), &c);
        let host = client(&rows, "Host");
        assert_eq!(host.header.value, "acme.example.com");
        assert_eq!(host.header.display, SECRET_MASK);
    }

    /// A token left in the userinfo doesn't hide a host that is known.
    #[test]
    fn header_rows_keep_host_when_only_the_userinfo_is_unresolved() {
        let rows =
            header_rows(&base("https://{{user}}@api.example.com/p"), &PrepareContext::default());
        assert_eq!(client(&rows, "Host").header.value, "api.example.com");
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
        assert!(!rows.iter().any(|r| r.header.name == "Host"));
        assert!(!rows.iter().any(|r| r.header.name == "Content-Length"));
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
            rows.iter().map(|r| r.header.name.as_str()).collect::<Vec<_>>(),
            ["X-D", "X-A", "Content-Type", "Accept", "Host", "Content-Length"]
        );
        assert_eq!(
            row(&rows, "X-A").origin,
            HeaderOrigin::Request { row: 1 },
            "the index counts the disabled row before it"
        );
    }

    /// A prepared request keeps the rows it sets apart from the whole
    /// wire, which adds the client's.
    #[test]
    fn prepared_headers_split_the_sent_rows_from_the_wire() {
        let mut req = base("https://x.test");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("Accept".into(), on("text/plain"));
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        let names = |rows: &[SentHeader]| rows.iter().map(|h| h.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&p.headers), ["Accept", "Content-Type"]);
        assert_eq!(
            names(&p.wire),
            ["accept", "content-type", "host", "content-length"],
            "wire order, wire case"
        );
    }

    /// The wire holds a name's values together, at the place the name first
    /// went in; the URL's `Authorization` goes in first, so a row that
    /// replaces it goes out first too.
    #[test]
    fn the_wire_keeps_a_names_values_together() {
        let names = |p: &PreparedRequest| {
            p.wire.iter().map(|h| format!("{}={}", h.name, h.value)).collect::<Vec<_>>()
        };
        let mut req = base("http://x.test/p");
        req.headers.insert("X-A".into(), on("1"));
        req.headers.insert("X-B".into(), on("2"));
        req.headers.insert("x-a".into(), on("3"));
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(
            names(&p),
            ["x-a=1", "x-a=3", "x-b=2", "accept=application/json", "host=x.test"]
        );

        let mut req = base("http://u:pw@x.test/p");
        req.headers.insert("X-One".into(), on("1"));
        req.headers.insert("Authorization".into(), on("t1"));
        req.headers.insert("authorization".into(), on("t2"));
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(
            names(&p),
            [
                "authorization=t1",
                "authorization=t2",
                "x-one=1",
                "accept=application/json",
                "host=x.test"
            ]
        );

        // A typed Accept goes out where it is written, the auto one not at
        // all.
        let mut req = base("http://x.test/p");
        req.headers.insert("X-One".into(), on("1"));
        req.headers.insert("Accept".into(), on("a"));
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(names(&p), ["x-one=1", "accept=a", "host=x.test"]);
    }

    /// A secret in a header's name is masked on screen like one in its
    /// value, and the real name is what goes on the wire.
    #[test]
    fn a_secret_in_a_header_name_is_masked_in_display_name() {
        let mut c = ctx(&[("key", "s3cret")], &[("X-{{key}}", "1", true)]);
        c.meta.insert("key".into(), varmodel::VarMeta::Secret);
        let rows = header_rows(&base("https://x.test"), &c);
        let r = row(&rows, "X-s3cret");
        assert_eq!(r.header.display_name, format!("X-{SECRET_MASK}"));
        let (p, _) = prepare(&base("https://x.test"), &c).unwrap();
        assert_eq!(wire(&p), vec![("X-s3cret".to_string(), "1".to_string()), json_accept()]);
    }

    /// A host with no secret written in it is sent and shown as it is,
    /// whatever its labels look like and wherever else a secret sits.
    #[test]
    fn a_host_without_a_secret_written_in_it_shows_as_sent() {
        let rows =
            header_rows(&base("https://postui-secret-mask.internal/x"), &PrepareContext::default());
        assert_eq!(client(&rows, "Host").header.value, "postui-secret-mask.internal");
        // Nor does a secret elsewhere in the URL mask it on screen.
        let mut c = ctx(&[("k", "s3cret")], &[]);
        c.meta.insert("k".into(), varmodel::VarMeta::Secret);
        let rows = header_rows(&base("https://postui-secret-mask.internal/x?key={{k}}"), &c);
        assert_eq!(client(&rows, "Host").header.display, "postui-secret-mask.internal");
    }

    /// The client turns the URL's userinfo into `Authorization: Basic …`,
    /// percent-decoded, before any row of ours — so it leads the list. A
    /// username that isn't UTF-8 gives none; a password that isn't is left
    /// off (reqwest's `extract_authority`).
    #[test]
    fn the_urls_userinfo_is_the_clients_authorization() {
        let auth = |url: &str| {
            let rows = header_rows(&base(url), &PrepareContext::default());
            let found = rows.iter().find(|r| r.header.name == "Authorization").cloned();
            if let Some(r) = &found {
                assert_eq!(r.origin, HeaderOrigin::Client, "{url}");
                assert!(r.sent(), "{url}");
                assert_eq!(rows[0], *r, "{url}: first on the wire");
                assert_eq!(r.header.display, r.header.value, "{url}: no secret to mask");
            }
            found.map(|r| r.header.value)
        };
        for (url, value) in [
            ("http://u:pw@x.test/p", "Basic dTpwdw=="),
            ("http://u@x.test/p", "Basic dTo="),
            ("http://u:@x.test/p", "Basic dTo="),
            ("http://:pw@x.test/p", "Basic OnB3"),
            ("http://u%40x:p%20w@x.test/p", "Basic dUB4OnAgdw=="),
            ("http://ü:é@x.test/p", "Basic w7w6w6k="),
            ("http://u:p%FF@x.test/p", "Basic dTo="),
        ] {
            assert_eq!(auth(url).as_deref(), Some(value), "{url}");
        }
        for url in ["http://x.test/p", "http://@x.test/p", "http://u%FF:p@x.test/p"] {
            assert_eq!(auth(url), None, "{url}");
        }
        assert_eq!(auth("http://{{user}}:pw@x.test/p"), None, "not while it is a token");
        assert_eq!(
            auth("http://u:%7B%7Bx@x.test/p{{gone}}").as_deref(),
            Some("Basic dTp7e3g="),
            "braces written in the userinfo are its text; a token elsewhere doesn't matter"
        );
    }

    /// A resolved value is the credential, braces and all: only a token
    /// still unresolved keeps the row back.
    #[test]
    fn a_resolved_credential_with_braces_is_still_listed() {
        let c = ctx(&[("pw", "ab{{cd")], &[]);
        let rows = header_rows(&base("http://u:{{pw}}@x.test/p"), &c);
        assert_eq!(client(&rows, "Authorization").header.value, basic_auth("u:ab{{cd"));
    }

    /// Like the auto `Accept`, the URL's `Authorization` stands aside for a sent row
    /// naming it, from the request or the defaults.
    #[test]
    fn a_sent_authorization_row_replaces_the_urls() {
        let mut req = base("http://u:pw@x.test/p");
        req.headers.insert("authorization".into(), on("Bearer t"));
        let rows = header_rows(&req, &PrepareContext::default());
        assert_eq!(client(&rows, "Authorization").unsent, Some(Unsent::Replaced));
        // The only Authorization on the wire: the client's Basic is gone.
        let auths = |p: &PreparedRequest| -> Vec<String> {
            p.wire.iter().filter(|h| h.name == "authorization").map(|h| h.value.clone()).collect()
        };
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(auths(&p), vec!["Bearer t".to_string()]);

        let req = base("http://u:pw@x.test/p");
        let c = ctx(&[], &[("Authorization", "Bearer d", true)]);
        let rows = header_rows(&req, &c);
        assert_eq!(client(&rows, "Authorization").unsent, Some(Unsent::Replaced));
        let (p, _) = prepare(&req, &c).unwrap();
        assert_eq!(auths(&p), vec!["Bearer d".to_string()]);
        let (p, _) = prepare(&req, &PrepareContext::default()).unwrap();
        assert_eq!(wire(&p), vec![json_accept()], "the client's, not the request's");
        assert_eq!(auths(&p), vec!["Basic dTpwdw==".to_string()]);
    }

    /// A secret that can change the URL's `Authorization` masks its
    /// credentials; one elsewhere in the URL leaves it as sent.
    #[test]
    fn a_secret_in_the_userinfo_masks_the_authorization() {
        let mut c = ctx(&[("pw", "hunter2"), ("k", "s3cret")], &[]);
        c.meta.insert("pw".into(), varmodel::VarMeta::Secret);
        c.meta.insert("k".into(), varmodel::VarMeta::Secret);
        let auth = |url: &str| client(&header_rows(&base(url), &c), "Authorization").header.clone();

        let h = auth("https://user:{{pw}}@x.test/p");
        assert_eq!(h.value, "Basic dXNlcjpodW50ZXIy");
        assert_eq!(h.display, format!("Basic {SECRET_MASK}"));
        let h = auth("https://user:pw@x.test/p?key={{k}}");
        assert_eq!(h.display, h.value, "the secret is not in the userinfo");
    }

    /// An empty body goes out with no `Content-Length`: the client sends
    /// one only for a body with bytes in it.
    #[test]
    fn an_empty_body_has_no_content_length() {
        for method in [Method::Post, Method::Put, Method::Patch, Method::Delete, Method::Options] {
            let mut req = base("http://x.test/p");
            req.method = method;
            req.body = Some(Body::Json { text: "".into() });
            req.headers.insert("Content-Length".into(), on("0"));
            let rows = header_rows(&req, &PrepareContext::default());
            assert!(
                !rows
                    .iter()
                    .any(|r| r.origin == HeaderOrigin::Client && r.header.name == "Content-Length")
            );
            assert_eq!(
                row(&rows, "Content-Length").unsent,
                Some(Unsent::ClientOwned { header: "Content-Length" }),
                "{method:?}"
            );
        }
        // Empty once substituted counts too.
        let mut req = base("http://x.test/p");
        req.method = Method::Post;
        req.substitute_body = true;
        req.body = Some(Body::Json { text: "{{payload}}".into() });
        let c = ctx(&[("payload", "")], &[]);
        let rows = header_rows(&req, &c);
        assert!(!rows.iter().any(|r| r.header.name == "Content-Length"));
        let (p, _) = prepare(&req, &c).unwrap();
        assert!(p.wire.iter().all(|h| h.name != "content-length"));
    }

    /// `Transfer-Encoding` is the client's too: it frames the body itself,
    /// and a typed one would be rewritten or dropped and take
    /// `Content-Length` with it. Listed, never sent; the client sends none.
    #[test]
    fn a_typed_transfer_encoding_is_never_sent() {
        let mut req = base("http://x.test/p");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("transfer-ENCODING".into(), on("chunked"));
        let c = ctx(&[], &[("Transfer-Encoding", "gzip", true)]);
        let rows = header_rows(&req, &c);
        let owned = Some(Unsent::ClientOwned { header: "Transfer-Encoding" });
        let from = |origin| rows.iter().find(|r| r.origin == origin).unwrap().unsent;
        assert_eq!(from(HeaderOrigin::Default), owned);
        assert_eq!(from(HeaderOrigin::Request { row: 0 }), owned);
        assert!(client(&rows, "Content-Length").sent());
        let (p, _) = prepare(&req, &c).unwrap();
        assert!(!p.wire.iter().any(|h| h.name == "transfer-encoding"));
    }

    /// A row the request may take over says how the request row that does
    /// so is written (task #20): a default with its `{{tokens}}` as
    /// written, the auto `Content-Type` and `Accept` as sent.
    /// The request's own rows, a row that isn't sent, the client-owned
    /// rows and the URL's `Authorization` have none.
    #[test]
    fn header_rows_say_how_an_overriding_row_is_written() {
        let mut req = base("https://user:pw@x.test/p");
        req.method = Method::Post;
        req.body = Some(Body::Json { text: "{}".into() });
        req.headers.insert("X-Mine".into(), on("1"));
        let c = ctx(
            &[("api_key", "s3cret")],
            &[("X-Key", "Bearer {{api_key}}", true), ("X-Mine", "0", true), ("Host", "h", true)],
        );
        let rows = header_rows(&req, &c);
        let overriding = |origin: HeaderOrigin, name: &str| {
            rows.iter()
                .find(|r| r.origin == origin && r.header.name.eq_ignore_ascii_case(name))
                .unwrap_or_else(|| panic!("no {origin:?} {name} row in {rows:#?}"))
                .overriding_row()
        };
        assert_eq!(
            overriding(HeaderOrigin::Default, "X-Key"),
            Some(("X-Key", "Bearer {{api_key}}")),
            "a default as written, its token unresolved"
        );
        assert_eq!(
            overriding(HeaderOrigin::Auto, "Content-Type"),
            Some(("Content-Type", "application/json"))
        );
        assert_eq!(overriding(HeaderOrigin::Auto, "Accept"), Some(("Accept", "application/json")));
        let mine = HeaderOrigin::Request { row: 0 };
        assert_eq!(overriding(mine, "X-Mine"), None, "already the request's");
        assert_eq!(overriding(HeaderOrigin::Default, "X-Mine"), None, "replaced, so not sent");
        assert_eq!(overriding(HeaderOrigin::Default, "Host"), None, "client-owned");
        for name in ["Authorization", "Host", "Content-Length"] {
            assert_eq!(overriding(HeaderOrigin::Client, name), None, "{name}");
        }
        // The field itself says so, sent or not, for a reader that
        // doesn't go through `overriding_row`.
        let never = |r: &&HeaderRow| r.override_as.is_none();
        for name in ["Content-Length", "Host", "X-Mine"] {
            assert!(rows.iter().filter(|r| r.header.name == name).all(|r| never(&r)), "{name}");
        }

        // A disabled request row leaves the auto row sent, but it is the
        // row to edit: the auto row offers none.
        req.headers.insert("accept".into(), off("text/plain"));
        let rows = header_rows(&req, &c);
        assert!(auto_accept(&rows).sent());
        assert_eq!(auto_accept(&rows).overriding_row(), None);
    }
}
