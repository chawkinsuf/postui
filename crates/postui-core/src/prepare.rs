use crate::model::{Body, Entry, HttpRequest, Method};
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
        }
    }
}

pub fn prepare(req: &HttpRequest) -> (PreparedRequest, Vec<PrepareWarning>) {
    let mut warnings = Vec::new();
    let enabled: Vec<(&String, &Entry)> = req.params.iter().filter(|(_, e)| e.enabled).collect();
    let url = if enabled.is_empty() {
        req.url.clone()
    } else {
        let (base, query) = match req.url.split_once('?') {
            Some((b, q)) => (b, q),
            None => (req.url.as_str(), ""),
        };
        // (key, value) pairs; URL pairs first, in order, before the
        // `[params]` table's entries are merged in below.
        let mut pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        for (k, e) in &enabled {
            let existing = pairs.iter().position(|(pk, _)| pk == *k);
            if let Some(i) = existing {
                warnings.push(PrepareWarning::ParamOverridesUrl { key: (*k).clone() });
                pairs.retain(|(pk, _)| pk != *k);
                pairs.insert(i.min(pairs.len()), ((*k).clone(), e.value.clone()));
            } else {
                pairs.push(((*k).clone(), e.value.clone()));
            }
        }
        let qs = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        format!("{base}?{qs}")
    };
    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|(_, e)| e.enabled)
        .map(|(k, e)| (k.clone(), e.value.clone()))
        .collect();
    let body = req.body.as_ref().map(|Body::Json { text }| text.clone());
    if body.is_some()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".into(), "application/json".into()));
    }
    (
        PreparedRequest {
            method: req.method,
            url,
            headers,
            body,
        },
        warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use indexmap::IndexMap;

    fn base(url: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url: url.into(),
            substitute_body: false,
            params: IndexMap::new(),
            headers: IndexMap::new(),
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
        let (p, warns) = prepare(&r);
        assert_eq!(p.url, "https://x.test/path?q=a+b%26c");
        assert!(warns.is_empty());
    }

    #[test]
    fn params_table_wins_over_url_query_with_warning() {
        let mut r = base("https://x.test/p?id=1&keep=y");
        r.params.insert("id".into(), on("2"));
        let (p, warns) = prepare(&r);
        assert_eq!(p.url, "https://x.test/p?id=2&keep=y");
        assert_eq!(
            warns,
            vec![PrepareWarning::ParamOverridesUrl { key: "id".into() }]
        );
    }

    #[test]
    fn url_literal_duplicates_are_kept_verbatim() {
        let (p, warns) = prepare(&base("https://x.test/p?id=1&id=2"));
        assert_eq!(p.url, "https://x.test/p?id=1&id=2");
        assert!(
            warns.is_empty(),
            "user-typed duplicates pass through untouched"
        );
    }

    #[test]
    fn url_without_params_table_is_untouched() {
        let (p, _) = prepare(&base("https://x.test/p?a=%20weird&b"));
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
        let (p, _) = prepare(&r);
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
        let (p, _) = prepare(&r);
        assert_eq!(p.headers.len(), 1);
        assert_eq!(p.headers[0].1, "application/vnd.x+json");
    }
}
