use crate::model::{Body, Entry, HttpRequest, Method};
use crate::varmodel;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
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
                write!(
                    f,
                    "query param `{}` in [params] overrides the one in the URL",
                    key
                )
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

pub fn prepare(
    req: &HttpRequest,
    ctx: &PrepareContext,
) -> Result<(PreparedRequest, Vec<PrepareWarning>), PrepareError> {
    // Request `[variables]` overlays the context's resolved vars at highest
    // precedence: enabled entries only, applied on a clone so the context
    // itself is untouched.
    let mut vars = ctx.vars.clone();
    for (k, e) in &req.variables {
        if e.enabled {
            vars.insert(k.clone(), e.value.clone());
        }
    }

    let mut missing = BTreeSet::new();
    let mut sub = |s: &str| crate::vars::substitute(s, &vars, &mut missing);

    let mut warnings = Vec::new();
    let subbed_url = sub(&req.url);
    let enabled: Vec<(String, String)> = req
        .params
        .iter()
        .filter(|(_, e)| e.enabled)
        .map(|(k, e)| (sub(k), sub(&e.value)))
        .collect();
    let url = if enabled.is_empty() {
        subbed_url
    } else {
        let (base, query) = match subbed_url.split_once('?') {
            Some((b, q)) => (b.to_string(), q.to_string()),
            None => (subbed_url.clone(), String::new()),
        };
        // (key, value) pairs; URL pairs first, in order, before the
        // `[params]` table's entries are merged in below.
        let mut pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
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
        let qs = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        format!("{base}?{qs}")
    };

    // Default headers merge UNDER the request's headers: an inherited
    // default is dropped when the request has any row (enabled or
    // disabled) whose name is case-insensitively equal to the default's
    // name AFTER {{var}} substitution — a templated default header name
    // (`X-{{t}}`) is suppressed by a request header matching the
    // substituted result. Two defaults that resolve to the same
    // lowercase name after substitution are deduped: the first wins and
    // the rest are dropped with a warning.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut seen_default_names: BTreeSet<String> = BTreeSet::new();
    for (k, e) in ctx.default_headers.iter() {
        if !e.enabled {
            continue;
        }
        let name = sub(k);
        if req.headers.keys().any(|rk| rk.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !seen_default_names.insert(lower.clone()) {
            warnings.push(PrepareWarning::DuplicateDefaultHeader { name: lower });
            continue;
        }
        headers.push((name, sub(&e.value)));
    }
    headers.extend(
        req.headers
            .iter()
            .filter(|(_, e)| e.enabled)
            .map(|(k, e)| (sub(k), sub(&e.value))),
    );

    let mut body = req.body.as_ref().map(|Body::Json { text }| text.clone());
    if req.substitute_body {
        body = body.map(|b| sub(&b));
    }

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

    if body.is_some()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".into(), "application/json".into()));
    }
    Ok((
        PreparedRequest {
            method: req.method,
            url,
            headers,
            body,
        },
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use indexmap::IndexMap;

    fn ctx(vars: &[(&str, &str)], defaults: &[(&str, &str, bool)]) -> PrepareContext {
        PrepareContext {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            default_headers: defaults
                .iter()
                .map(|(k, v, en)| {
                    (
                        k.to_string(),
                        Entry {
                            value: v.to_string(),
                            enabled: *en,
                        },
                    )
                })
                .collect(),
            meta: IndexMap::new(),
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
        assert!(p.headers.contains(&("x-trace".into(), "on".into())));
    }

    #[test]
    fn body_substitution_is_opt_in() {
        let mut r = base("http://x.test");
        r.body = Some(Body::Json {
            text: r#"{"t": "{{tok}}"}"#.into(),
        });
        let c = ctx(&[("tok", "abc")], &[]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            p.body.as_deref(),
            Some(r#"{"t": "{{tok}}"}"#),
            "flag off: literal braces"
        );
        r.substitute_body = true;
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(p.body.as_deref(), Some(r#"{"t": "abc"}"#));
    }

    #[test]
    fn unresolved_variables_error_and_body_tokens_only_count_when_opted_in() {
        let mut r = base("http://x.test/{{gone}}");
        r.body = Some(Body::Json {
            text: "{{also_gone}}".into(),
        });
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
            &[
                ("accept", "application/json", true),
                ("x-trace", "1", true),
                ("x-off", "0", false),
            ],
        );
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            p.headers,
            vec![("Accept".to_string(), "text/plain".to_string())],
            "override kept in request position; suppressed + disabled defaults dropped"
        );
    }

    #[test]
    fn default_header_values_are_substituted_too() {
        let r = base("http://x.test");
        let c = ctx(
            &[("tok", "abc")],
            &[("authorization", "Bearer {{tok}}", true)],
        );
        let (p, _) = prepare(&r, &c).unwrap();
        assert!(
            p.headers
                .contains(&("authorization".into(), "Bearer abc".into()))
        );
    }

    #[test]
    fn default_header_suppression_compares_names_after_substitution() {
        let mut r = base("http://x.test");
        r.headers.insert("x-api".into(), on("literal"));
        let c = ctx(&[("t", "Api")], &[("X-{{t}}", "default-value", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert_eq!(
            p.headers,
            vec![("x-api".to_string(), "literal".to_string())],
            "substituted default name X-Api matches request header x-api case-insensitively"
        );
    }

    #[test]
    fn default_header_suppression_after_substitution_honors_disabled_row() {
        let mut r = base("http://x.test");
        r.headers.insert("x-api".into(), off("ignored"));
        let c = ctx(&[("t", "Api")], &[("X-{{t}}", "default-value", true)]);
        let (p, _) = prepare(&r, &c).unwrap();
        assert!(
            p.headers.is_empty(),
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
        };
        let (p, warns) = prepare(&base("http://x.test"), &c).unwrap();
        assert_eq!(
            p.headers,
            vec![("X-One".to_string(), "first".to_string())],
            "first duplicate wins"
        );
        assert_eq!(
            warns,
            vec![PrepareWarning::DuplicateDefaultHeader {
                name: "x-one".to_string()
            }]
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
        c.meta
            .insert("user".into(), varmodel::VarMeta::NeedsSelection);
        c.meta
            .insert("api_key".into(), varmodel::VarMeta::MissingSecret);
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
            method: Method::Get,
            url: url.into(),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
            variables: IndexMap::new(),
            body: None,
        }
    }
    fn on(v: &str) -> Entry {
        Entry {
            value: v.into(),
            enabled: true,
        }
    }
    fn off(v: &str) -> Entry {
        Entry {
            value: v.into(),
            enabled: false,
        }
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
    fn params_table_wins_over_url_query_with_warning() {
        let mut r = base("https://x.test/p?id=1&keep=y");
        r.params.insert("id".into(), on("2"));
        let (p, warns) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.url, "https://x.test/p?id=2&keep=y");
        assert_eq!(
            warns,
            vec![PrepareWarning::ParamOverridesUrl { key: "id".into() }]
        );
    }

    #[test]
    fn url_literal_duplicates_are_kept_verbatim() {
        let (p, warns) = prepare(
            &base("https://x.test/p?id=1&id=2"),
            &PrepareContext::default(),
        )
        .unwrap();
        assert_eq!(p.url, "https://x.test/p?id=1&id=2");
        assert!(
            warns.is_empty(),
            "user-typed duplicates pass through untouched"
        );
    }

    #[test]
    fn url_without_params_table_is_untouched() {
        let (p, _) = prepare(
            &base("https://x.test/p?a=%20weird&b"),
            &PrepareContext::default(),
        )
        .unwrap();
        assert_eq!(
            p.url, "https://x.test/p?a=%20weird&b",
            "no table entries: never rewrite the query"
        );
    }

    #[test]
    fn headers_filter_disabled_and_json_body_auto_adds_content_type() {
        let mut r = base("https://x.test");
        r.headers.insert("A".into(), on("1"));
        r.headers.insert("B".into(), off("2"));
        r.body = Some(Body::Json { text: "{}".into() });
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(
            p.headers,
            vec![
                ("A".into(), "1".into()),
                ("Content-Type".into(), "application/json".into())
            ]
        );
        assert_eq!(p.body.as_deref(), Some("{}"));
    }

    #[test]
    fn explicit_content_type_wins_case_insensitively() {
        let mut r = base("https://x.test");
        r.headers
            .insert("content-TYPE".into(), on("application/vnd.x+json"));
        r.body = Some(Body::Json { text: "{}".into() });
        let (p, _) = prepare(&r, &PrepareContext::default()).unwrap();
        assert_eq!(p.headers.len(), 1);
        assert_eq!(p.headers[0].1, "application/vnd.x+json");
    }
}
