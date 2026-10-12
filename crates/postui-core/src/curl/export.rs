//! A prepared request as a curl command (spec §5.3), and whether that
//! command carries a secret (§5.4).

use crate::model::{Body, HttpRequest, Method};
use crate::prepare::{PrepareContext, PreparedRequest};
use crate::varmodel::VarMeta;

/// `req` as a bash/zsh command line: one flag per line joined with ` \`
/// continuations, every argument single-quoted (spec R4). The first line
/// is `curl [-X METHOD | --head] 'URL' [-g] [-k]`: `-g` (D9) only when the
/// URL holds any of `[ ] { }`, `-k` only for an insecure request.
pub fn to_curl(req: &PreparedRequest) -> String {
    let mut head = String::from("curl");
    match req.method {
        Method::Get => {}
        // `-X HEAD` makes curl wait for a body that never comes.
        Method::Head => head.push_str(" --head"),
        m => {
            head.push_str(" -X ");
            head.push_str(m.as_str());
        }
    }
    head.push(' ');
    head.push_str(&quote(&req.url));
    // curl reads `[…]` and `{…}` in a URL as globs; `-g` keeps them text.
    if req.url.contains(['[', ']', '{', '}']) {
        head.push_str(" -g");
    }
    if req.insecure {
        head.push_str(" -k");
    }
    let mut lines = vec![head];
    for (name, value) in &req.headers {
        // `Name:` with nothing after it tells curl to drop the header;
        // `Name;` is curl's spelling of an empty value.
        let header = if value.is_empty() {
            format!("{name};")
        } else {
            format!("{name}: {value}")
        };
        lines.push(format!("-H {}", quote(&header)));
    }
    if let Some(body) = &req.body {
        // `--data-raw`, not `-d`: a body starting with `@` stays text.
        lines.push(format!("--data-raw {}", quote(body)));
    }
    lines.join(" \\\n  ")
}

/// POSIX single quotes: everything inside is literal, and `'` itself is
/// spelled `'\''`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Whether a secret variable's value went into `prepared` — what the
/// "includes secret values" note on the copy toast reports (spec §5.4).
/// The URL and headers carry a masked twin to compare against; the body
/// has none, so its `{{…}}` tokens are checked by name.
pub fn includes_secrets(req: &HttpRequest, ctx: &PrepareContext, prepared: &PreparedRequest) -> bool {
    if prepared.url != prepared.display_url || prepared.headers != prepared.display_headers {
        return true;
    }
    let Some(Body::Json { text }) = &req.body else {
        return false;
    };
    req.substitute_body
        && prepared.body.is_some()
        && crate::vars::find_tokens(text)
            .iter()
            .any(|t| ctx.meta.get(&t.name) == Some(&VarMeta::Secret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HttpRequest, Method};
    use crate::prepare::{PrepareContext, PreparedRequest, prepare};
    use crate::varmodel::VarMeta;

    fn prepared(method: Method, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> PreparedRequest {
        let headers: Vec<(String, String)> =
            headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        PreparedRequest {
            method,
            url: url.into(),
            display_url: url.into(),
            display_headers: headers.clone(),
            headers,
            body: body.map(str::to_string),
            insecure: false,
        }
    }

    #[test]
    fn a_get_is_bare() {
        assert_eq!(to_curl(&prepared(Method::Get, "https://x/a?b=1", &[], None)), "curl 'https://x/a?b=1'");
    }

    #[test]
    fn one_flag_per_line_headers_in_order_body_as_data_raw() {
        let p = prepared(
            Method::Post,
            "https://api.example.com/users?page=2",
            &[("Authorization", "Bearer abc123"), ("Content-Type", "application/json")],
            Some(r#"{"name": "x"}"#),
        );
        assert_eq!(
            to_curl(&p),
            "curl -X POST 'https://api.example.com/users?page=2' \\\n  -H 'Authorization: Bearer abc123' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{\"name\": \"x\"}'"
        );
    }

    #[test]
    fn head_is_the_head_flag_never_x_head() {
        assert_eq!(to_curl(&prepared(Method::Head, "https://x", &[], None)), "curl --head 'https://x'");
    }

    #[test]
    fn insecure_follows_the_url() {
        let mut p = prepared(Method::Delete, "https://x/1", &[], None);
        p.insecure = true;
        assert_eq!(to_curl(&p), "curl -X DELETE 'https://x/1' -k");
    }

    #[test]
    fn brackets_or_braces_in_the_url_add_globoff_before_insecure() {
        assert_eq!(
            to_curl(&prepared(Method::Get, "https://x/items?page[size]=10", &[], None)),
            "curl 'https://x/items?page[size]=10' -g"
        );
        for url in ["https://x/a]", "https://x/{a,b}", "https://x/a}", "https://x/{a"] {
            assert!(to_curl(&prepared(Method::Get, url, &[], None)).ends_with("' -g"), "{url}");
        }
        let mut p = prepared(Method::Get, "https://x/items?page[size]=10", &[], None);
        p.insecure = true;
        assert_eq!(to_curl(&p), "curl 'https://x/items?page[size]=10' -g -k");
    }

    #[test]
    fn a_url_without_brackets_has_no_globoff() {
        assert!(!to_curl(&prepared(Method::Get, "https://x/a?b=(1)", &[], None)).contains("-g"));
    }

    #[test]
    fn single_quotes_are_escaped_everywhere() {
        let p = prepared(Method::Put, "https://x/it's", &[("X-Note", "it's")], Some("it's"));
        assert_eq!(
            to_curl(&p),
            "curl -X PUT 'https://x/it'\\''s' \\\n  -H 'X-Note: it'\\''s' \\\n  --data-raw 'it'\\''s'"
        );
    }

    #[test]
    fn a_body_starting_with_at_and_a_dollar_stay_literal() {
        let p = prepared(Method::Post, "https://x", &[], Some("@file $HOME"));
        assert!(to_curl(&p).ends_with("--data-raw '@file $HOME'"));
    }

    #[test]
    fn an_empty_header_value_uses_curls_semicolon_form() {
        let p = prepared(Method::Get, "https://x", &[("X-Empty", "")], None);
        assert_eq!(to_curl(&p), "curl 'https://x' \\\n  -H 'X-Empty;'");
    }

    fn secret_ctx() -> PrepareContext {
        let mut ctx = PrepareContext::default();
        ctx.vars.insert("token".into(), "s3cret".into());
        ctx.meta.insert("token".into(), VarMeta::Secret);
        ctx.vars.insert("plain".into(), "hello".into());
        ctx
    }

    fn secret_check(toml: &str) -> bool {
        let req = HttpRequest::from_toml_str(toml).unwrap();
        let ctx = secret_ctx();
        let (p, _) = prepare(&req, &ctx).unwrap();
        includes_secrets(&req, &ctx, &p)
    }

    #[test]
    fn a_secret_in_a_header_or_url_is_reported() {
        assert!(secret_check("url = \"https://x\"\n[headers]\nAuthorization = \"Bearer {{token}}\"\n"));
        assert!(secret_check("url = \"https://x/{{token}}\"\n"));
        assert!(!secret_check("url = \"https://x/{{plain}}\"\n[headers]\nX = \"{{plain}}\"\n"));
    }

    #[test]
    fn a_secret_in_the_body_counts_only_when_the_body_is_substituted() {
        let body = "method = \"POST\"\nurl = \"https://x\"\n{}[body]\ntype = \"json\"\ntext = \"{{token}}\"\n";
        assert!(secret_check(&body.replace("{}", "substitute_body = true\n")));
        assert!(!secret_check(&body.replace("{}", "")));
    }
}
