use crate::checks::Measure;
use indexmap::IndexMap;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
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

impl Entry {
    /// The entry as a table row in a file: enabled as a plain string,
    /// disabled as `{ value, enabled = false }`.
    pub(crate) fn to_item(&self) -> toml_edit::Item {
        use toml_edit::{InlineTable, Item, Value};
        if self.enabled {
            return toml_edit::value(self.value.as_str());
        }
        let mut inline = InlineTable::new();
        inline.insert("value", Value::from(self.value.as_str()));
        inline.insert("enabled", Value::from(false));
        Item::Value(Value::InlineTable(inline))
    }
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
                Ok(Entry { value: v.to_string(), enabled: true })
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Entry { value: v, enabled: true })
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
                Ok(Entry { value, enabled: enabled.unwrap_or(true) })
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

/// A named jq filter saved in the request file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JqView {
    pub name: String,
    pub filter: String,
    /// In-memory identity (Editor assigns it); never read or written.
    #[serde(skip)]
    pub id: u64,
}

/// The filter a request's response pane applies: view `current`'s, else the
/// `scratch`. The editor's bar and `run --view` both ask this.
pub fn applied_jq_filter<'a>(
    views: &'a [JqView],
    current: Option<&str>,
    scratch: &'a str,
) -> &'a str {
    current
        .and_then(|name| views.iter().find(|v| v.name == name))
        .map_or(scratch, |v| v.filter.as_str())
}

impl JqView {
    /// What two view names are compared by: trimmed, ignoring case
    /// (Unicode, as request names are). One request never holds two views
    /// with the same key (spec 18).
    pub fn name_key(name: &str) -> String {
        name.trim().to_lowercase()
    }

    /// Why `name` may not name a view beside `others` (the other views'
    /// names), or `None`: blank, holding a control character (a tab, an
    /// escape: the chip and the menu paint the name as it is), or another's
    /// by [`Self::name_key`] (spec 18). The one rule the file reader and the
    /// editor's save-as and rename apply, so the app never writes a name its
    /// reader refuses.
    pub fn name_refusal<'a>(
        name: &str,
        others: impl IntoIterator<Item = &'a str>,
    ) -> Option<ViewNameRefusal> {
        let key = Self::name_key(name);
        if key.is_empty() {
            return Some(ViewNameRefusal::Blank);
        }
        if name.chars().any(char::is_control) {
            return Some(ViewNameRefusal::Control);
        }
        others.into_iter().any(|o| Self::name_key(o) == key).then_some(ViewNameRefusal::Taken)
    }
}

/// See [`JqView::name_refusal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewNameRefusal {
    Blank,
    Control,
    Taken,
}

impl PartialEq for JqView {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.filter == other.filter
    }
}

impl Eq for JqView {}

/// See the serialization warning on [`Entry`]: the derived `Serialize` here
/// emits `[params.*]`/`[headers.*]` sub-table sections and is not
/// order-preserving. Persist requests with [`HttpRequest::to_toml_string`],
/// never `toml::to_string(&req)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The scratch jq filter: the request's unnamed view, which cannot be
    /// renamed or deleted (jq views spec, amendment A1). `None`/empty is no
    /// filter and is omitted from the TOML. View-only: never affects what
    /// is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jq: Option<String>,
    /// Whether the jq filter is switched on (the bar's switch). `true` is
    /// the default and is omitted; `false` is written as it is, whatever
    /// there is to filter by, so the file holds what the editor does
    /// (amendment A5).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub jq_enabled: bool,
    /// The saved jq views, written to the request file as `[[jq_view]]`.
    /// A name the app would refuse (blank, a control character, or two
    /// alike, trimmed and ignoring case) refuses the read: the app names a
    /// view by its name, as the TOML names a header by its key.
    #[serde(
        default,
        rename = "jq_view",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "valid_view_names"
    )]
    pub jq_views: Vec<JqView>,
    /// The current view's name; `None` is the scratch, and is omitted. A
    /// name no view has reads as the scratch ([`Self::from_toml_str`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jq_current: Option<String>,
    /// What `postui suite` and `postui run` check the response against
    /// (`[checks]`, suite checks spec 2026-11-14), edited on the app's
    /// Checks tab. postui 0.4.1 and older refuse a file with this table
    /// (an unknown key): the cost named when checks were put in the
    /// request file (spec §2.1, R1).
    #[serde(default, skip_serializing_if = "Checks::is_empty")]
    pub checks: Checks,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub params: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
}

/// A request's `[checks]` (suite checks spec 2026-11-14 §2.1): what its
/// response must be for `suite` and `run` to pass it. Evaluated by
/// [`crate::checks::evaluate`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checks {
    /// The statuses that pass: a response passes on any term (status
    /// ranges spec 2026-11-19, S1), one term or a list in the file. Empty
    /// is the default rule, below 400 (R3).
    #[serde(default, deserialize_with = "status_terms", skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<StatusTerm>,
    /// The most milliseconds the response head may take to come in (spec
    /// §2.7, T1); `None` checks nothing.
    #[serde(default, deserialize_with = "limit_ms", skip_serializing_if = "Option::is_none")]
    pub max_ttfb_ms: Option<u32>,
    /// The most milliseconds the whole response may take, body read (T1).
    #[serde(default, deserialize_with = "limit_ms", skip_serializing_if = "Option::is_none")]
    pub max_elapsed_ms: Option<u32>,
    /// jq filters that must each give `true` and nothing else (R4).
    #[serde(default, deserialize_with = "filters", skip_serializing_if = "Vec::is_empty")]
    pub jq: Vec<String>,
}

impl Checks {
    pub fn is_empty(&self) -> bool {
        self.status.is_empty()
            && self.max_ttfb_ms.is_none()
            && self.max_elapsed_ms.is_none()
            && self.jq.is_empty()
    }

    /// The status terms typed as `200, 201-204, < 400` (status ranges spec
    /// S4): commas or spaces between terms, while a space after a
    /// comparison's sign or beside a range's `-` belongs to its term. Empty
    /// text is no status check. A term typed twice counts once, as in the
    /// file. The file read calls this on a string (S7).
    pub fn parse_status(text: &str) -> Result<Vec<StatusTerm>, String> {
        let mut terms = Vec::new();
        for word in status_words(text) {
            let term = StatusTerm::parse(&word)?;
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
        Ok(terms)
    }

    /// A time limit typed as a whole number of milliseconds, by the
    /// read's own rule; empty text is no limit.
    pub fn parse_limit(text: &str) -> Result<Option<u32>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        text.parse::<u32>().ok().and_then(limit_of).map(Some).ok_or_else(|| {
            format!(
                "a check's time limit must be a whole number of milliseconds from 1 to {}, \
                 not {}",
                u32::MAX,
                crate::checks::brief(text)
            )
        })
    }
}

/// `v` when it is a status code a check may name: 100 to 599.
fn status_code<V: TryInto<u16>>(v: V) -> Option<u16> {
    v.try_into().ok().filter(|c| (100..=599).contains(c))
}

/// `v` when it is a time limit a check may hold: 1 to `u32::MAX`.
fn limit_of<V: TryInto<u32>>(v: V) -> Option<u32> {
    v.try_into().ok().filter(|ms| *ms > 0)
}

/// One term of a status check (status ranges spec 2026-11-19, S2). Every
/// number in it is a code from 100 to 599 (S3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTerm {
    /// `404`
    Code(u16),
    /// `200-299`: both ends included, the low one first.
    Range(u16, u16),
    /// `< 400`
    Below(u16),
    /// `<= 399`
    AtMost(u16),
    /// `> 499`
    Above(u16),
    /// `>= 500`
    AtLeast(u16),
}

impl StatusTerm {
    /// Whether a response with `status` passes on this term. A status
    /// outside 100 to 599 is judged by the same comparison (S14).
    pub fn matches(self, status: u16) -> bool {
        match self {
            StatusTerm::Code(code) => status == code,
            StatusTerm::Range(low, high) => (low..=high).contains(&status),
            StatusTerm::Below(n) => status < n,
            StatusTerm::AtMost(n) => status <= n,
            StatusTerm::Above(n) => status > n,
            StatusTerm::AtLeast(n) => status >= n,
        }
    }

    /// The term as the file writes it, as [`Serialize`] gives it (S6).
    fn to_value(self) -> toml_edit::Value {
        match self {
            StatusTerm::Code(code) => i64::from(code).into(),
            term => term.to_string().into(),
        }
    }

    /// One term as [`status_words`] gives it, read by S2's forms: a sign
    /// then a code, a code `-` a code, or a code, with a space allowed only
    /// after the sign and beside the `-`. A code is plain digits, 100 to
    /// 599. The refusal is S11's words.
    fn parse(word: &str) -> Result<Self, String> {
        let code = |n: &str| {
            Some(n)
                .filter(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse::<u16>().ok())
                .and_then(status_code)
                .ok_or_else(|| no_term(word))
        };
        let term = if let Some(n) = word.strip_prefix("<=") {
            StatusTerm::AtMost(code(n.trim_start())?)
        } else if let Some(n) = word.strip_prefix('<') {
            StatusTerm::Below(code(n.trim_start())?)
        } else if let Some(n) = word.strip_prefix(">=") {
            StatusTerm::AtLeast(code(n.trim_start())?)
        } else if let Some(n) = word.strip_prefix('>') {
            StatusTerm::Above(code(n.trim_start())?)
        } else if let Some((low, high)) = word.split_once('-') {
            let (low, high) = (code(low.trim_end())?, code(high.trim_start())?);
            if low > high {
                let word = crate::checks::brief(word);
                return Err(format!("a check's status range must run low to high, not {word}"));
            }
            StatusTerm::Range(low, high)
        } else {
            StatusTerm::Code(code(word)?)
        };
        if !(100..=599).any(|status| term.matches(status)) {
            return Err(format!("a check's status must match a code from 100 to 599, not {term}"));
        }
        Ok(term)
    }
}

/// S11's refusal of `word`, which is no term.
fn no_term(word: &str) -> String {
    format!(
        "a check's status must be codes from 100 to 599, ranges like 200-299 or comparisons \
         like < 400, not {}",
        crate::checks::brief(word)
    )
}

/// The terms of a status field's `text`, each as typed with its spaces cut
/// to one (S4): split at commas, then at spaces, except where a part ends
/// in a comparison's sign or a `-`, or the next one starts with a `-` and
/// the part is digits alone (a range's low end).
fn status_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for part in text.split(',') {
        let mut word = String::new();
        for piece in part.split_whitespace() {
            let joins = word.ends_with(['<', '>', '=', '-'])
                || (piece.starts_with('-') && word.bytes().all(|b| b.is_ascii_digit()));
            if !word.is_empty() && !joins {
                words.push(std::mem::take(&mut word));
            }
            if !word.is_empty() {
                word.push(' ');
            }
            word.push_str(piece);
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    words
}

/// S5's form: `404`, `200-299`, `< 400`, `<= 399`, `> 499`, `>= 500`.
impl fmt::Display for StatusTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusTerm::Code(code) => write!(f, "{code}"),
            StatusTerm::Range(low, high) => write!(f, "{low}-{high}"),
            StatusTerm::Below(n) => write!(f, "< {n}"),
            StatusTerm::AtMost(n) => write!(f, "<= {n}"),
            StatusTerm::Above(n) => write!(f, "> {n}"),
            StatusTerm::AtLeast(n) => write!(f, ">= {n}"),
        }
    }
}

/// As the file writes it (S6, S10): a code as a number, any other term as
/// its [`Display`](fmt::Display) string.
impl Serialize for StatusTerm {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            StatusTerm::Code(code) => s.serialize_u16(*code),
            term => s.collect_str(term),
        }
    }
}

/// `jq = [...]` as the app's list holds it (checks tabs spec 2026-11-16):
/// each filter without the whitespace around it, and one listed twice
/// read once, the first kept. The read is the one `run` and `suite` use,
/// so they list the check once too. The app's save writes the whole file
/// from the request it holds ([`HttpRequest::to_toml_string`]), so it
/// writes the list this way, as it writes no comment back; neither a
/// repeat nor the whitespace changed a verdict.
fn filters<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let mut seen = std::collections::HashSet::new();
    let filters = Vec::<String>::deserialize(d)?.into_iter().map(|f| f.trim().to_string());
    Ok(filters.filter(|f| seen.insert(f.clone())).collect())
}

/// `status = 200`, `status = "< 400"` or `status = ["200-299", 404]`
/// (status ranges spec S6, S7): a number is a code from 100 to 599, and a
/// string reads as the status field reads its text
/// ([`Checks::parse_status`]). Any other value refuses the read (R6). A
/// term listed twice reads once.
fn status_terms<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<StatusTerm>, D::Error> {
    /// The value's terms; `in_list` for an entry of the list, which may
    /// not be a list itself.
    struct Terms {
        in_list: bool,
    }

    fn code<E: de::Error, V: TryInto<u16> + fmt::Display + Copy>(
        v: V,
    ) -> Result<Vec<StatusTerm>, E> {
        status_code(v)
            .map(|c| vec![StatusTerm::Code(c)])
            .ok_or_else(|| E::custom(no_term(&v.to_string())))
    }

    impl<'de> DeserializeSeed<'de> for Terms {
        type Value = Vec<StatusTerm>;

        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(self)
        }
    }

    impl<'de> Visitor<'de> for Terms {
        type Value = Vec<StatusTerm>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a status code, a range or a comparison")?;
            if !self.in_list {
                f.write_str(", or a list of them")?;
            }
            Ok(())
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            code(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            code(v)
        }

        // Blank text reads as no terms, as in the field (S13) and as
        // `status = []` does; the app writes neither.
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Checks::parse_status(v).map_err(E::custom)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            if self.in_list {
                return Err(de::Error::invalid_type(de::Unexpected::Seq, &self));
            }
            let mut terms = Vec::new();
            while let Some(entry) = seq.next_element_seed(Terms { in_list: true })? {
                for term in entry {
                    if !terms.contains(&term) {
                        terms.push(term);
                    }
                }
            }
            Ok(terms)
        }
    }

    d.deserialize_any(Terms { in_list: false })
}

/// `max_ttfb_ms = 500`: a time limit, a whole number of milliseconds from 1
/// to `u32::MAX`; any other value refuses the read (spec §2.7, T2).
fn limit_ms<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    struct Limit;

    fn limit<E: de::Error, V: TryInto<u32> + fmt::Display + Copy>(v: V) -> Result<u32, E> {
        limit_of(v).ok_or_else(|| {
            E::custom(format!(
                "a check's time limit must be a whole number of milliseconds from 1 to {}, \
                 not {v}",
                u32::MAX
            ))
        })
    }

    impl<'de> Visitor<'de> for Limit {
        type Value = u32;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a whole number of milliseconds from 1 to {}", u32::MAX)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            limit(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            limit(v)
        }
    }

    d.deserialize_any(Limit).map(Some)
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// The file holds only names the app would accept (spec 18): none blank,
/// none with a control character, no two alike.
fn valid_view_names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<JqView>, D::Error> {
    let views = Vec::<JqView>::deserialize(d)?;
    for (i, v) in views.iter().enumerate() {
        let earlier = views[..i].iter().map(|w| w.name.as_str());
        let why = match JqView::name_refusal(&v.name, earlier) {
            None => continue,
            Some(ViewNameRefusal::Blank) => "a jq view has a blank name".to_string(),
            Some(ViewNameRefusal::Control) => {
                format!("the jq view name {:?} holds a control character", v.name)
            }
            Some(ViewNameRefusal::Taken) => {
                format!("two jq views are named {:?}", v.name.trim())
            }
        };
        return Err(serde::de::Error::custom(why));
    }
    Ok(views)
}

impl HttpRequest {
    /// Reads a request file. `jq_current` names its view as views are
    /// compared ([`JqView::name_key`]), and reads as that view's own name; one
    /// that names no view (a merge, a hand edit) reads as the scratch (spec
    /// 27, amendment A6). Either way the request opens clean; its next save
    /// writes the view's own name, or drops the line. A hand-written
    /// `jq = ""` reads as no scratch, as the writer never writes one.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        let mut req: Self = toml::from_str(s)?;
        if req.jq.as_deref() == Some("") {
            req.jq = None;
        }
        req.jq_current = req.jq_current.as_deref().and_then(|current| {
            let key = JqView::name_key(current);
            req.jq_views.iter().find(|v| JqView::name_key(&v.name) == key).map(|v| v.name.clone())
        });
        Ok(req)
    }

    /// Hand-built with toml_edit for exact formatting control: enabled entries
    /// as plain strings, disabled as *inline* tables (sub-table sections would
    /// force reordering), body text verbatim.
    pub fn to_toml_string(&self) -> String {
        use toml_edit::{Array, DocumentMut, Item, Table, Value, value};
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
        let scratch = self.jq.as_deref().filter(|s| !s.is_empty());
        if let Some(jq) = scratch {
            doc["jq"] = value(jq);
        }
        if !self.jq_enabled {
            doc["jq_enabled"] = value(false);
        }
        if let Some(current) = &self.jq_current {
            doc["jq_current"] = value(current);
        }
        if !self.jq_views.is_empty() {
            let mut views = toml_edit::ArrayOfTables::new();
            for v in &self.jq_views {
                let mut t = Table::new();
                t["name"] = value(v.name.as_str());
                t["filter"] = value(v.filter.as_str());
                views.push(t);
            }
            doc["jq_view"] = Item::ArrayOfTables(views);
        }
        if !self.checks.is_empty() {
            let mut t = Table::new();
            match self.checks.status.as_slice() {
                [] => {}
                [one] => t["status"] = value(one.to_value()),
                many => {
                    t["status"] = value(many.iter().map(|t| t.to_value()).collect::<Array>());
                }
            }
            for measure in Measure::ALL {
                if let Some(ms) = measure.limit(&self.checks) {
                    t[measure.key()] = value(i64::from(ms));
                }
            }
            if !self.checks.jq.is_empty() {
                // One filter a line: they are read, and diffed, one by one.
                let mut filters = Array::new();
                for filter in &self.checks.jq {
                    let mut v = Value::from(filter.as_str());
                    v.decor_mut().set_prefix("\n  ");
                    filters.push_formatted(v);
                }
                filters.set_trailing_comma(true);
                filters.set_trailing("\n");
                t["jq"] = value(filters);
            }
            doc["checks"] = Item::Table(t);
        }
        let kv_table = |map: &IndexMap<String, Entry>| {
            let mut t = Table::new();
            for (k, e) in map {
                t[k] = e.to_item();
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
    use super::StatusTerm::{Above, AtLeast, AtMost, Below, Code, Range};
    use super::*;
    use indexmap::IndexMap;

    fn sample() -> HttpRequest {
        let mut params = IndexMap::new();
        params.insert("page".into(), Entry { value: "2".into(), enabled: true });
        params.insert("verbose".into(), Entry { value: "1".into(), enabled: false });
        let mut headers = IndexMap::new();
        headers
            .insert("Authorization".into(), Entry { value: "Bearer abc123".into(), enabled: true });
        HttpRequest {
            name: None,
            method: Method::Post,
            url: "https://api.example.com/users".into(),
            substitute_body: false,
            insecure: false,
            jq: None,
            jq_enabled: true,
            jq_views: Vec::new(),
            jq_current: None,
            checks: Checks::default(),
            params,
            headers,
            variables: IndexMap::new(),
            body: Some(Body::Json { text: "{ \"broken\": ".into() }), // invalid JSON must round-trip
        }
    }

    #[test]
    fn display_name_round_trips_and_leads_the_document() {
        let mut req = sample();
        req.name = Some("Get user by ID!".into());
        let out = req.to_toml_string();
        assert!(out.starts_with("name = \"Get user by ID!\"\n"), "name leads the file:\n{out}");
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
        assert!(out.contains(r#"page = "2""#), "enabled entry is a plain string:\n{out}");
        assert!(out.contains("verbose = {"), "disabled entry is an inline table:\n{out}");
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
        assert_eq!(req.headers["A"], Entry { value: "1".into(), enabled: true });
        assert_eq!(req.headers["B"], Entry { value: "2".into(), enabled: false });
        assert_eq!(req.headers["C"], Entry { value: "3".into(), enabled: true });
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
        assert!(msg.contains("reserved"), "array rejection names the reservation: {msg}");
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
    fn views_round_trip_as_an_array_of_tables_after_the_plain_keys() {
        let mut req = sample();
        req.insecure = true;
        req.jq_views = vec![
            JqView { name: "names".into(), filter: ".items[].name".into(), id: 0 },
            JqView {
                name: "active users".into(),
                filter: ".items[] | select(.active)".into(),
                id: 0,
            },
        ];
        let out = req.to_toml_string();
        let insecure_at = out.find("insecure = true").unwrap();
        let first =
            out.find("[[jq_view]]\nname = \"names\"\nfilter = \".items[].name\"").expect(&out);
        assert!(insecure_at < first, "{out}");
        assert!(first < out.find("[params]").unwrap(), "{out}");
        assert_eq!(out.matches("[[jq_view]]").count(), 2, "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
    }

    #[test]
    fn no_views_writes_no_jq_view_table() {
        assert!(!sample().to_toml_string().contains("jq_view"));
    }

    #[test]
    fn jq_defaults_none_and_is_omitted_from_toml() {
        let req = sample();
        assert_eq!(req.jq, None);
        let out = req.to_toml_string();
        assert!(!out.contains("jq"), "no jq line:\n{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap().jq, None);
    }

    /// Amendment A3–A6: the scratch, the switch and the current view are
    /// plain keys after `insecure`, before the views' tables.
    #[test]
    fn the_scratch_switch_and_current_view_round_trip_before_the_views() {
        let mut req = sample();
        req.insecure = true;
        req.jq = Some(".data | length".into());
        req.jq_enabled = false;
        req.jq_views = vec![JqView { name: "names".into(), filter: ".n".into(), id: 0 }];
        req.jq_current = Some("names".into());
        let out = req.to_toml_string();
        let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s}:\n{out}"));
        let order = [
            at("insecure = true"),
            at("jq = \".data | length\""),
            at("jq_enabled = false"),
            at("jq_current = \"names\""),
            at("[[jq_view]]"),
            at("[params]"),
        ];
        assert!(order.is_sorted(), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
    }

    /// Amendment A5: off is written as it is, so the file round-trips what
    /// the editor holds even with nothing to filter.
    #[test]
    fn jq_enabled_defaults_true_and_off_is_always_written() {
        let mut req = sample();
        assert!(req.jq_enabled);
        req.jq_enabled = false;
        let out = req.to_toml_string();
        assert!(out.contains("jq_enabled = false"), "even with nothing to filter:\n{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
        req.jq = Some(".a".into());
        assert_eq!(HttpRequest::from_toml_str(&req.to_toml_string()).unwrap(), req);
        req.jq_enabled = true;
        assert!(!req.to_toml_string().contains("jq_enabled"), "on is the default");
    }

    #[test]
    fn a_hand_written_empty_jq_reads_as_no_scratch() {
        let back =
            HttpRequest::from_toml_str("method = \"GET\"\nurl = \"u\"\njq = \"\"\n").unwrap();
        assert_eq!(back.jq, None);
    }

    /// Amendment A6: a current view the file does not have reads as the
    /// scratch.
    #[test]
    fn a_jq_current_naming_no_view_reads_as_the_scratch() {
        let back = HttpRequest::from_toml_str(
            "method = \"GET\"\nurl = \"u\"\njq_current = \"gone\"\n\n[[jq_view]]\nname = \"here\"\nfilter = \".\"\n",
        )
        .unwrap();
        assert_eq!(back.jq_current, None);
        assert_eq!(back.jq_views.len(), 1);

        let cased = HttpRequest::from_toml_str(
            "method = \"GET\"\nurl = \"u\"\njq_current = \" Here\"\n\n[[jq_view]]\nname = \"here\"\nfilter = \".\"\n",
        )
        .unwrap();
        assert_eq!(cased.jq_current.as_deref(), Some("here"), "matched as names compare");
    }

    #[test]
    fn two_views_with_one_name_refuse_the_read() {
        let text = "method = \"GET\"\nurl = \"u\"\n\n[[jq_view]]\nname = \"Ärger\"\nfilter = \".a\"\n\n[[jq_view]]\nname = \" ärger \"\nfilter = \".b\"\n";
        let err = HttpRequest::from_toml_str(text).unwrap_err();
        assert!(err.to_string().contains("two jq views are named \"ärger\""), "{err}");
    }

    #[test]
    fn a_blank_view_name_refuses_the_read() {
        let view = |name: &str| {
            format!(
                "method = \"GET\"\nurl = \"u\"\n\n[[jq_view]]\nname = \"{name}\"\nfilter = \".\"\n"
            )
        };
        let blank = HttpRequest::from_toml_str(&view("  ")).unwrap_err();
        assert!(blank.to_string().contains("a jq view has a blank name"), "{blank}");
        // Amendment C9: no name is reserved, the chip's own included.
        for name in ["Scratch", "no view"] {
            assert!(HttpRequest::from_toml_str(&view(name)).is_ok(), "{name}");
        }
        let tab = HttpRequest::from_toml_str(&view("a\\tb")).unwrap_err();
        assert!(tab.to_string().contains("holds a control character"), "{tab}");
    }

    #[test]
    fn view_equality_ignores_the_in_memory_id() {
        let a = JqView { name: "n".into(), filter: ".".into(), id: 1 };
        let b = JqView { id: 7, ..a.clone() };
        assert_eq!(a, b);
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
            Entry { value: "http://override.test".into(), enabled: true },
        );
        req.variables.insert("token".into(), Entry { value: "shh".into(), enabled: false });
        let out = req.to_toml_string();

        assert!(
            out.contains(r#"base_url = "http://override.test""#),
            "enabled entry is a plain string:\n{out}"
        );
        assert!(out.contains("token = {"), "disabled entry is an inline table:\n{out}");
        assert!(!out.contains("[variables.token]"), "no sub-table sections:\n{out}");

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
        assert!(!out.contains("[variables]"), "empty variables table is skipped:\n{out}");
    }

    /// Suite checks spec §2.1, §4: `[checks]` round-trips after the jq
    /// views and before `[params]`, one status as a number and several as a
    /// list, each filter on its own line.
    #[test]
    fn checks_round_trip_after_the_views_before_params() {
        let mut req = sample();
        req.jq_views = vec![JqView { name: "names".into(), filter: ".n".into(), id: 0 }];
        req.checks = Checks {
            status: vec![Code(200)],
            jq: vec![".a == 1".into(), ".choices[0].finish_reason == \"stop\"".into()],
            ..Checks::default()
        };
        let out = req.to_toml_string();
        let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s}:\n{out}"));
        assert!(at("[[jq_view]]") < at("[checks]") && at("[checks]") < at("[params]"), "{out}");
        assert!(out.contains("[checks]\nstatus = 200\njq = [\n  \".a == 1\",\n"), "{out}");
        assert!(out.contains("\n  '.choices[0].finish_reason == \"stop\"',\n]\n"), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);

        req.checks.status = vec![Code(200), Code(201)];
        let out = req.to_toml_string();
        assert!(out.contains("status = [200, 201]"), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
    }

    /// Status ranges spec S6, S7: a range or a comparison is written as a
    /// string in S5's form, a code as a number, one term bare and several
    /// as a list, and each reads back.
    #[test]
    fn a_status_range_or_comparison_round_trips_as_a_string() {
        let mut req = sample();
        let write = |req: &HttpRequest| {
            let out = req.to_toml_string();
            assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), *req, "{out}");
            out
        };
        req.checks.status = vec![Range(200, 299), Code(404)];
        assert!(write(&req).contains("[checks]\nstatus = [\"200-299\", 404]\n"));
        req.checks.status = vec![Below(400)];
        assert!(write(&req).contains("[checks]\nstatus = \"< 400\"\n"));
        req.checks.status = vec![AtMost(399), Above(499), AtLeast(500), Code(200)];
        let out = write(&req);
        assert!(out.contains("status = [\"<= 399\", \"> 499\", \">= 500\", 200]\n"), "{out}");
    }

    #[test]
    fn no_checks_write_nothing_and_old_files_read_without_them() {
        let out = sample().to_toml_string();
        assert!(!out.contains("checks"), "{out}");
        assert!(HttpRequest::from_toml_str(&out).unwrap().checks.is_empty());
        let empty = HttpRequest::from_toml_str("url = \"u\"\n[checks]\n").unwrap();
        assert!(empty.checks.is_empty());
        assert!(!empty.to_toml_string().contains("checks"));
    }

    #[test]
    fn checks_read_one_status_or_a_list() {
        let read =
            |checks: &str| HttpRequest::from_toml_str(&format!("url = \"u\"\n[checks]\n{checks}"));
        assert_eq!(read("status = 404\n").unwrap().checks.status, [Code(404)]);
        assert_eq!(read("status = [200, 204]\n").unwrap().checks.status, [Code(200), Code(204)]);
        assert_eq!(read("jq = ['.a']\n").unwrap().checks.jq, [".a"]);
    }

    /// Status ranges spec S7: a string reads as the status field reads its
    /// text, so it may hold a code, a range or a comparison, spaced either
    /// way; a list may mix them with numbers.
    #[test]
    fn a_status_string_reads_as_the_field_reads_it() {
        let status = |checks: &str| {
            HttpRequest::from_toml_str(&format!("url = \"u\"\n[checks]\n{checks}"))
                .unwrap()
                .checks
                .status
        };
        assert_eq!(status("status = \"200\"\n"), [Code(200)]);
        // Blank text is no status check, as in the field and as `[]` is.
        assert!(status("status = \"\"\n").is_empty());
        assert_eq!(status("status = \"<400\"\n"), [Below(400)]);
        assert_eq!(status("status = [\"200 - 299\", 404]\n"), [Range(200, 299), Code(404)]);
        assert_eq!(
            status("status = [\"200\", 200, \"< 400\", \"<400\"]\n"),
            [Code(200), Below(400)]
        );
    }

    /// Spec R6, status ranges spec S7, S11: a status that isn't a code from
    /// 100 to 599, a range or a comparison refuses the read with the
    /// field's words; so does a key `[checks]` doesn't know.
    #[test]
    fn a_bad_check_status_or_key_refuses_the_read() {
        let read =
            |checks: &str| HttpRequest::from_toml_str(&format!("url = \"u\"\n[checks]\n{checks}"));
        let terms = "a check's status must be codes from 100 to 599, ranges like 200-299 or \
                     comparisons like < 400, not ";
        for (bad, why) in [
            ("status = 99\n", format!("{terms}99")),
            ("status = 600\n", format!("{terms}600")),
            ("status = [200, 1000]\n", format!("{terms}1000")),
            ("status = -1\n", format!("{terms}-1")),
            ("status = \"2xx\"\n", format!("{terms}2xx")),
            ("status = [\"200-\"]\n", format!("{terms}200-")),
            ("status = \"< 600\"\n", format!("{terms}< 600")),
            ("status = \"200-+299\"\n", format!("{terms}200-+299")),
            ("status = [\"> = 500\"]\n", format!("{terms}> = 500")),
            (
                "status = \"300-200\"\n",
                "a check's status range must run low to high, not 300-200".into(),
            ),
            (
                "status = [\"< 100\"]\n",
                "a check's status must match a code from 100 to 599, not < 100".into(),
            ),
            (
                "status = \">599\"\n",
                "a check's status must match a code from 100 to 599, not > 599".into(),
            ),
        ] {
            let err = read(bad).unwrap_err().to_string();
            assert!(err.contains(&why), "{bad}: {err}");
        }
        for bad in
            ["status = 200.0\n", "status = [[200]]\n", "status = { a = 1 }\n", "status = true\n"]
        {
            assert!(read(bad).is_err(), "{bad}");
        }
        for bad in ["jq = '.a'\n", "header = 1\n"] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }

    /// Spec §2.7 (T2): the time limits round-trip after `status` and before
    /// `jq`, each alone too.
    #[test]
    fn time_limits_round_trip_between_status_and_jq() {
        let mut req = sample();
        req.checks = Checks {
            status: vec![Code(200)],
            max_ttfb_ms: Some(500),
            max_elapsed_ms: Some(2000),
            jq: vec![".a == 1".into()],
        };
        let out = req.to_toml_string();
        assert!(
            out.contains(
                "[checks]\nstatus = 200\nmax_ttfb_ms = 500\nmax_elapsed_ms = 2000\njq = [\n"
            ),
            "{out}"
        );
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);

        req.checks = Checks { max_elapsed_ms: Some(1), ..Checks::default() };
        assert!(!req.checks.is_empty());
        let out = req.to_toml_string();
        assert!(out.contains("[checks]\nmax_elapsed_ms = 1\n"), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);

        req.checks = Checks { max_ttfb_ms: Some(250), ..Checks::default() };
        let out = req.to_toml_string();
        assert!(out.contains("[checks]\nmax_ttfb_ms = 250\n"), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);

        // The largest limit is written as it is.
        req.checks = Checks { max_elapsed_ms: Some(u32::MAX), ..Checks::default() };
        let out = req.to_toml_string();
        assert!(out.contains("[checks]\nmax_elapsed_ms = 4294967295\n"), "{out}");
        assert_eq!(HttpRequest::from_toml_str(&out).unwrap(), req);
    }

    /// Checks tabs spec 2026-11-16: a status code or a filter listed twice
    /// reads once, the first kept, so nothing downstream sees a repeat. A
    /// filter reads without the whitespace around it.
    #[test]
    fn a_repeated_check_reads_once() {
        let read = HttpRequest::from_toml_str(
            "url = \"u\"\n[checks]\nstatus = [200, 201, 200]\njq = ['.a', ' .b', '.a ']\n",
        )
        .unwrap();
        assert_eq!(read.checks.status, [Code(200), Code(201)]);
        assert_eq!(read.checks.jq, [".a", ".b"]);
    }

    /// A field's text reads by the file's own limits (Q5).
    #[test]
    fn a_typed_status_or_limit_reads_by_the_files_rules() {
        assert_eq!(Checks::parse_status(""), Ok(vec![]));
        assert_eq!(Checks::parse_status("200"), Ok(vec![Code(200)]));
        assert_eq!(
            Checks::parse_status(" 200, 201,200 404 "),
            Ok(vec![Code(200), Code(201), Code(404)])
        );
        // A number is plain digits: no sign, no leading zero; a sign has no
        // space inside it.
        for bad in [
            "99", "600", "2xx", "200;201", "-200", "200.5", "<", "<=", "200-", "--", "+200",
            "0200", "<+400", ">=0500", "200-+299", "200-0299", "< = 399", "> = 500", "=< 400",
        ] {
            let err = Checks::parse_status(bad).unwrap_err();
            let want = "a check's status must be codes from 100 to 599, ranges like 200-299 or \
                        comparisons like < 400, not ";
            assert_eq!(err, format!("{want}{bad}"));
        }
        // Status ranges spec S3, S11: a range the wrong way round, and a
        // term no code from 100 to 599 matches, each with its own words.
        let wrong = "a check's status range must run low to high, not ";
        assert_eq!(Checks::parse_status("300-200"), Err(format!("{wrong}300-200")));
        assert_eq!(Checks::parse_status("200, 300  - 200"), Err(format!("{wrong}300 - 200")));
        let none = "a check's status must match a code from 100 to 599, not ";
        assert_eq!(Checks::parse_status("<100"), Err(format!("{none}< 100")));
        assert_eq!(Checks::parse_status("> 599"), Err(format!("{none}> 599")));
        assert_eq!(Checks::parse_status("<= 100"), Ok(vec![AtMost(100)]));
        assert_eq!(Checks::parse_status(">=599"), Ok(vec![AtLeast(599)]));
        // A `-` after a finished term starts a term of its own.
        let terms = "a check's status must be codes from 100 to 599, ranges like 200-299 or \
                     comparisons like < 400, not ";
        assert_eq!(Checks::parse_status(">= 500 -200"), Err(format!("{terms}-200")));
        assert_eq!(Checks::parse_status("< 400 - 200"), Err(format!("{terms}- 200")));
        assert_eq!(Checks::parse_limit(" "), Ok(None));
        assert_eq!(Checks::parse_limit(" 500 "), Ok(Some(500)));
        assert_eq!(Checks::parse_limit("4294967295"), Ok(Some(u32::MAX)));
        for bad in ["0", "-1", "1.5", "500 ms", "4294967296", "soon"] {
            let err = Checks::parse_limit(bad).unwrap_err();
            assert!(err.starts_with("a check's time limit must be a whole number"), "{err}");
        }
        // What was typed is quoted cut short.
        let long = "9".repeat(200);
        let range = format!("300-{long}");
        let errs = [&long, &range].map(|typed| Checks::parse_status(typed));
        for err in errs.into_iter().chain([Checks::parse_limit(&long).map(|_| vec![])]) {
            let err = err.unwrap_err();
            let quoted = err.rsplit_once(", not ").unwrap().1;
            assert!(quoted.ends_with('…') && quoted.chars().count() == 60, "{err}");
        }
    }

    /// Status ranges spec S2, S4, S5: each term form reads with or without
    /// spaces inside it, between terms commas or spaces; each shows back in
    /// one form; a term typed twice counts once and overlaps both stay.
    #[test]
    fn each_status_term_reads_and_shows_in_one_form() {
        let cases = [
            ("404", Code(404), "404"),
            ("200-299", Range(200, 299), "200-299"),
            ("200 - 299", Range(200, 299), "200-299"),
            ("200 -299", Range(200, 299), "200-299"),
            ("200- 299", Range(200, 299), "200-299"),
            ("200-200", Range(200, 200), "200-200"),
            ("<400", Below(400), "< 400"),
            ("< 400", Below(400), "< 400"),
            ("<=399", AtMost(399), "<= 399"),
            ("<= 399", AtMost(399), "<= 399"),
            (">499", Above(499), "> 499"),
            ("> 499", Above(499), "> 499"),
            (">=500", AtLeast(500), ">= 500"),
            (">=  500", AtLeast(500), ">= 500"),
        ];
        for (typed, term, shown) in cases {
            assert_eq!(Checks::parse_status(typed), Ok(vec![term]), "{typed}");
            assert_eq!(term.to_string(), shown, "{typed}");
        }
        assert_eq!(
            Checks::parse_status("200, 200-299 < 400,<400 ,  >= 500 404"),
            Ok(vec![Code(200), Range(200, 299), Below(400), AtLeast(500), Code(404)])
        );
    }

    /// Status ranges spec S1, S14: a term matches the codes it names, both
    /// ends of a range included, a status past 599 by the same comparison.
    #[test]
    fn a_status_term_matches_at_its_edges() {
        let cases: [(StatusTerm, &[u16], &[u16]); 6] = [
            (Code(404), &[404], &[403, 405]),
            (Range(200, 299), &[200, 250, 299], &[199, 300]),
            (Below(400), &[100, 399], &[400, 999]),
            (AtMost(399), &[399], &[400]),
            (Above(499), &[500, 600], &[499]),
            (AtLeast(500), &[500, 999], &[499]),
        ];
        for (term, pass, fail) in cases {
            assert!(pass.iter().all(|s| term.matches(*s)), "{term} {pass:?}");
            assert!(!fail.iter().any(|s| term.matches(*s)), "{term} {fail:?}");
        }
    }

    /// `Checks` reads back what its `Serialize` gives in a format that hands
    /// a positive integer to `visit_u64`, as JSON does; a term other than a
    /// code goes as its string (status ranges spec S10).
    #[test]
    fn checks_round_trip_through_json() {
        let checks = Checks {
            status: vec![Code(200), Range(201, 204), Below(400)],
            max_ttfb_ms: Some(500),
            max_elapsed_ms: Some(2000),
            jq: vec![".a".into()],
        };
        let json = serde_json::to_string(&checks).unwrap();
        assert!(json.starts_with(r#"{"status":[200,"201-204","< 400"],"#), "{json}");
        assert_eq!(serde_json::from_str::<Checks>(&json).unwrap(), checks);
    }

    /// Spec §2.7 (T2): a limit that isn't a whole number of milliseconds
    /// from 1 to `u32::MAX` refuses the read, as a bad `status` does (R6).
    #[test]
    fn a_bad_time_limit_refuses_the_read() {
        let read =
            |checks: &str| HttpRequest::from_toml_str(&format!("url = \"u\"\n[checks]\n{checks}"));
        for key in ["max_ttfb_ms", "max_elapsed_ms"] {
            for bad in ["0", "-5", "4294967296", "1.5", "\"500\"", "\"500ms\"", "[500]", "true"] {
                let err = read(&format!("{key} = {bad}\n")).unwrap_err().to_string();
                assert!(
                    err.contains("a whole number of milliseconds from 1 to 4294967295"),
                    "{bad}: {err}"
                );
            }
            assert_eq!(
                read(&format!("{key} = 1\n")).unwrap().checks,
                match key {
                    "max_ttfb_ms" => Checks { max_ttfb_ms: Some(1), ..Checks::default() },
                    _ => Checks { max_elapsed_ms: Some(1), ..Checks::default() },
                }
            );
        }
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
