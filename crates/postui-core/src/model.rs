use indexmap::IndexMap;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }

    /// Whether a request with this method puts its body on the wire. GET
    /// and HEAD requests keep any body text *stored* (it's still there when
    /// the method switches back) but never send it.
    pub fn sends_body(self) -> bool {
        !matches!(self, Method::Get | Method::Head)
    }

    pub fn cycle(self) -> Method {
        let i = Method::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Method::ALL[(i + 1) % Method::ALL.len()]
    }
}

/// # Serialization warning
///
/// `Entry`'s and `HttpRequest`'s derived/hand-rolled `Serialize` impls exist
/// for round-tripping through `serde`-based paths (e.g. tests); their TOML
/// output uses `[params.foo]`/`[headers.foo]` *sub-table* sections, which
/// reorders keys when re-serialized. Do not use `toml::to_string` (or
/// anything that goes through `Serialize`) to persist a request to disk.
/// [`HttpRequest::to_toml_string`] is the only canonical writer: it builds
/// the document by hand with `toml_edit` so enabled entries stay plain
/// strings, disabled entries stay inline tables, and insertion order is
/// preserved exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub value: String,
    pub enabled: bool,
}

impl Serialize for Entry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.enabled {
            serializer.serialize_str(&self.value)
        } else {
            let mut s = serializer.serialize_struct("Entry", 2)?;
            s.serialize_field("value", &self.value)?;
            s.serialize_field("enabled", &self.enabled)?;
            s.end()
        }
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = Entry;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or a table with `value` and optional `enabled`")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Entry {
                    value: v.to_string(),
                    enabled: true,
                })
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Entry {
                    value: v,
                    enabled: true,
                })
            }

            fn visit_seq<A>(self, _seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                Err(de::Error::custom(
                    "array values are reserved for a future version; use a single string or { value = \"…\", enabled = false }",
                ))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut value: Option<String> = None;
                let mut enabled: Option<bool> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "value" => {
                            if value.is_some() {
                                return Err(de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        "enabled" => {
                            if enabled.is_some() {
                                return Err(de::Error::duplicate_field("enabled"));
                            }
                            enabled = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, &["value", "enabled"]));
                        }
                    }
                }
                let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
                Ok(Entry {
                    value,
                    enabled: enabled.unwrap_or(true),
                })
            }
        }

        deserializer.deserialize_any(EntryVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Body {
    Json { text: String },
}

/// See the serialization warning on [`Entry`]: the derived `Serialize` here
/// emits `[params.*]`/`[headers.*]` sub-table sections and is not
/// order-preserving. Persist requests with [`HttpRequest::to_toml_string`],
/// never `toml::to_string(&req)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    /// The user-facing display name of this request (free-form; spaces and
    /// punctuation welcome). The filename is a slug *derived* from it —
    /// never typed and never shown — so `None` (legacy files) simply
    /// displays as the slug leaf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub method: Method,
    pub url: String,
    /// Whether `{{var}}` tokens in the body are substituted at send time.
    /// Opt-in per request; `false` is the default and is omitted from the
    /// TOML so untouched requests don't churn in diffs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub substitute_body: bool,
    /// Whether TLS certificate verification is skipped when sending this
    /// request (curl's `-k`/`--insecure`). Opt-in per request; `false` is
    /// the default and is omitted from the TOML.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    /// The response pane's jq filter for this request (spec: jq response
    /// filter). View-only — never affects what is sent. `None`/empty is no
    /// filter and is omitted from the TOML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jq: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub params: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
}

impl HttpRequest {
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Hand-built with toml_edit for exact formatting control: enabled entries
    /// as plain strings, disabled as *inline* tables (sub-table sections would
    /// force reordering), body text verbatim.
    pub fn to_toml_string(&self) -> String {
        use toml_edit::{DocumentMut, Item, Table, Value, value};
        let mut doc = DocumentMut::new();
        if let Some(name) = &self.name {
            doc["name"] = value(name);
        }
        doc["method"] = value(self.method.as_str());
        doc["url"] = value(&self.url);
        if self.substitute_body {
            doc["substitute_body"] = value(true);
        }
        if self.insecure {
            doc["insecure"] = value(true);
        }
        if let Some(jq) = self.jq.as_deref().filter(|s| !s.is_empty()) {
            doc["jq"] = value(jq);
        }
        let kv_table = |map: &IndexMap<String, Entry>| {
            let mut t = Table::new();
            for (k, e) in map {
                if e.enabled {
                    t[k] = value(&e.value);
                } else {
                    let mut inline = toml_edit::InlineTable::new();
                    inline.insert("value", Value::from(e.value.as_str()));
                    inline.insert("enabled", Value::from(false));
                    t[k] = Item::Value(Value::InlineTable(inline));
                }
            }
            Item::Table(t)
        };
        if !self.params.is_empty() {
            doc["params"] = kv_table(&self.params);
        }
        if !self.headers.is_empty() {
            doc["headers"] = kv_table(&self.headers);
        }
        if !self.variables.is_empty() {
            doc["variables"] = kv_table(&self.variables);
        }
        if let Some(Body::Json { text }) = &self.body {
            let mut t = Table::new();
            t["type"] = value("json");
            t["text"] = value(text.as_str());
            doc["body"] = Item::Table(t);
        }
        doc.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn sample() -> HttpRequest {
        let mut params = IndexMap::new();
        params.insert(
            "page".into(),
            Entry {
                value: "2".into(),
                enabled: true,
            },
        );
        params.insert(
            "verbose".into(),
            Entry {
                value: "1".into(),
                enabled: false,
            },
        );
        let mut headers = IndexMap::new();
        headers.insert(
            "Authorization".into(),
            Entry {
                value: "Bearer abc123".into(),
                enabled: true,
            },
        );
        HttpRequest {
            name: None,
            method: Method::Post,
            url: "https://api.example.com/users".into(),
            substitute_body: false,
            insecure: false,
            jq: None,
            params,
            headers,
            variables: IndexMap::new(),
            body: Some(Body::Json {
                text: "{ \"broken\": ".into(),
            }), // invalid JSON must round-trip
        }
    }

    #[test]
    fn display_name_round_trips_and_leads_the_document() {
        let mut req = sample();
        req.name = Some("Get user by ID!".into());
        let out = req.to_toml_string();
        assert!(
            out.starts_with("name = \"Get user by ID!\"\n"),
            "name leads the file:\n{out}"
        );
        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert_eq!(back.name.as_deref(), Some("Get user by ID!"));
    }

    #[test]
    fn nameless_request_emits_no_name_line_and_parses_as_none() {
        let out = sample().to_toml_string();
        assert!(!out.contains("name ="), "no name line:\n{out}");
        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert_eq!(back.name, None);
    }

    #[test]
    fn round_trips_preserving_content_and_order() {
        let req = sample();
        let toml_str = req.to_toml_string();
        let back = HttpRequest::from_toml_str(&toml_str).unwrap();
        assert_eq!(back, req);
        let keys: Vec<&String> = back.params.keys().collect();
        assert_eq!(keys, ["page", "verbose"], "insertion order preserved");
    }

    #[test]
    fn enabled_entries_serialize_as_plain_strings_disabled_as_inline_tables() {
        let out = sample().to_toml_string();
        assert!(
            out.contains(r#"page = "2""#),
            "enabled entry is a plain string:\n{out}"
        );
        assert!(
            out.contains("verbose = {"),
            "disabled entry is an inline table:\n{out}"
        );
        assert!(
            !out.contains("[params.verbose]"),
            "no sub-table sections (they break ordering):\n{out}"
        );
    }

    #[test]
    fn parses_string_and_table_entry_forms() {
        let req = HttpRequest::from_toml_str(
            r#"
        method = "GET"
        url = "https://x.test"
        [headers]
        A = "1"
        B = { value = "2", enabled = false }
        C = { value = "3", enabled = true }
    "#,
        )
        .unwrap();
        assert_eq!(
            req.headers["A"],
            Entry {
                value: "1".into(),
                enabled: true
            }
        );
        assert_eq!(
            req.headers["B"],
            Entry {
                value: "2".into(),
                enabled: false
            }
        );
        assert_eq!(
            req.headers["C"],
            Entry {
                value: "3".into(),
                enabled: true
            }
        );
    }

    #[test]
    fn missing_method_defaults_to_get_missing_sections_default_empty() {
        let req = HttpRequest::from_toml_str(r#"url = "https://x.test""#).unwrap();
        assert_eq!(req.method, Method::Get);
        assert!(req.params.is_empty() && req.headers.is_empty() && req.body.is_none());
    }

    #[test]
    fn rejects_unknown_keys_arrays_and_bad_entries() {
        assert!(
            HttpRequest::from_toml_str(
                r#"url = "u"
        bogus = 1"#
            )
            .is_err(),
            "unknown top-level key"
        );
        let arr = HttpRequest::from_toml_str(
            r#"url = "u"
        [params]
        id = ["1", "2"]"#,
        );
        let msg = arr.unwrap_err().to_string();
        assert!(
            msg.contains("reserved"),
            "array rejection names the reservation: {msg}"
        );
        assert!(
            HttpRequest::from_toml_str(
                r#"url = "u"
        [headers]
        A = { value = "1", typo = true }"#
            )
            .is_err(),
            "unknown entry field"
        );
        assert!(
            HttpRequest::from_toml_str(
                r#"url = "u"
        [body]
        type = "yaml"
        text = "x""#
            )
            .is_err(),
            "unknown body type"
        );
    }

    #[test]
    fn insecure_defaults_false_and_is_omitted_from_toml() {
        let req = sample();
        assert!(!req.insecure);
        let out = req.to_toml_string();
        assert!(!out.contains("insecure"), "no insecure line:\n{out}");
        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert!(!back.insecure);
    }

    #[test]
    fn insecure_true_round_trips() {
        let mut req = sample();
        req.insecure = true;
        let out = req.to_toml_string();
        assert!(out.contains("insecure = true"), "emitted when true:\n{out}");
        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert!(back.insecure);
        assert_eq!(back, req);
    }

    #[test]
    fn jq_defaults_none_and_is_omitted_from_toml() {
        let req = sample();
        assert_eq!(req.jq, None);
        let out = req.to_toml_string();
        assert!(!out.contains("jq"), "no jq line:\n{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap().jq, None);
    }

    #[test]
    fn jq_round_trips_after_insecure_and_before_the_tables() {
        let mut req = sample();
        req.insecure = true;
        req.jq = Some(".data | length".into());
        let out = req.to_toml_string();
        let insecure_at = out.find("insecure = true").unwrap();
        let jq_at = out
            .find("jq = \".data | length\"")
            .expect("jq line:\n{out}");
        let params_at = out.find("[params]").unwrap_or(usize::MAX);
        assert!(insecure_at < jq_at && jq_at < params_at, "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
    }

    #[test]
    fn method_cycles_through_all_and_wraps() {
        let mut m = Method::Get;
        for _ in 0..Method::ALL.len() {
            m = m.cycle();
        }
        assert_eq!(m, Method::Get);
        assert_eq!(Method::Delete.as_str(), "DELETE");
    }

    #[test]
    fn variables_round_trip_with_disabled_inline_entry_after_headers_before_body() {
        let mut req = sample();
        req.variables.insert(
            "base_url".into(),
            Entry {
                value: "http://override.test".into(),
                enabled: true,
            },
        );
        req.variables.insert(
            "token".into(),
            Entry {
                value: "shh".into(),
                enabled: false,
            },
        );
        let out = req.to_toml_string();

        assert!(
            out.contains(r#"base_url = "http://override.test""#),
            "enabled entry is a plain string:\n{out}"
        );
        assert!(
            out.contains("token = {"),
            "disabled entry is an inline table:\n{out}"
        );
        assert!(
            !out.contains("[variables.token]"),
            "no sub-table sections:\n{out}"
        );

        let headers_pos = out.find("[headers]").expect("headers section present");
        let variables_pos = out.find("[variables]").expect("variables section present");
        let body_pos = out.find("[body]").expect("body section present");
        assert!(
            headers_pos < variables_pos && variables_pos < body_pos,
            "[variables] must come after [headers] and before [body]:\n{out}"
        );

        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn empty_variables_table_is_omitted() {
        let out = sample().to_toml_string();
        assert!(
            !out.contains("[variables]"),
            "empty variables table is skipped:\n{out}"
        );
    }

    #[test]
    fn substitute_body_round_trips_and_is_omitted_when_false() {
        let mut req = sample();
        assert!(!req.substitute_body, "default off");
        let out = req.to_toml_string();
        assert!(!out.contains("substitute_body"), "false is omitted: {out}");

        req.substitute_body = true;
        let out = req.to_toml_string();
        assert!(out.contains("substitute_body = true"), "{out}");
        let back = HttpRequest::from_toml_str(&out).unwrap();
        assert!(back.substitute_body);
        assert_eq!(back, req);
    }
}
