//! A structure-only outline of a JSON body for the AI prompt: key names
//! and types, never values.

use crate::jq::path::key_label;
use serde_json::Value;
use std::rc::Rc;

#[derive(Debug, Clone, Copy)]
pub struct ShapeLimits {
    pub max_bytes: usize,
}

impl Default for ShapeLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024,
        }
    }
}

/// The outline handed to the model, and one full jq path through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub outline: String,
    pub example_path: Option<String>,
}

/// The whole structure, one key per line, indented under its parent:
/// scalars become type names, every element of an array is merged into
/// one description (a key some elements lack is marked `key?`), and an
/// array is `name[]`. A wide object whose values all share their kinds
/// (`FOLD_KEYS`) is one line listing its key names, with its values merged
/// under `[]`. No
/// depth limit; only the total size is capped, and a cut says how many
/// lines it dropped. Non-JSON → `None`.
pub fn shape(body: &str, limits: ShapeLimits) -> Option<Shape> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut schema = Schema::default();
    schema.add(&value);
    schema.collapse();
    let mut outline = Outline::new(limits.max_bytes);
    let mut path = Vec::new();
    outline.render(None, &schema, 0, &mut path);
    let (text, kept) = outline.text();
    // The example must name keys the model can see: the deepest leaf among
    // the lines that survived the cut, first found on a tie.
    let example_path = outline
        .leaves
        .iter()
        .filter(|(line, _)| *line < kept)
        .max_by(|(la, a), (lb, b)| a.len().cmp(&b.len()).then(lb.cmp(la)))
        .map(|(_, segs)| jq_path(segs));
    Some(Shape {
        outline: text,
        example_path,
    })
}

/// Everything seen at one position of the document, merged.
#[derive(Default)]
struct Schema {
    null: bool,
    boolean: bool,
    number: bool,
    /// `Some(tag)`: strings were seen, and `tag` is the format they all
    /// share (`None` once two disagree).
    string: Option<Option<&'static str>>,
    object: Option<Object>,
    /// The merged elements of every array seen here.
    array: Option<Box<Schema>>,
}

/// An object with more distinct keys than this, every value of the same
/// kinds (null aside; string formats included) and, for object values,
/// mostly the same keys (`values_share_keys`), is folded: its key names go
/// on one line (`key?` when some of the merged objects lack it) and its
/// values merge under one `[]`. Listed one per line, a lookup table keyed by
/// data (currency codes, dates, ids) would bury everything after it; folded,
/// a wide record still shows every name it can.
const FOLD_KEYS: usize = 20;

/// The most bytes a folded object's key list takes before `… N more`.
const FOLD_LIST_BYTES: usize = 400;

/// The most keys an object lists one per line before `… N more keys`: a
/// wide object the fold rule passes over (values of several kinds) would
/// otherwise push every key after it past the byte cap.
const LIST_KEYS: usize = 100;

#[derive(Default)]
struct Object {
    /// How many objects were merged, to tell which keys some lack.
    seen: usize,
    fields: Vec<Field>,
    index: std::collections::HashMap<Rc<str>, usize>,
    /// Set by `collapse` in place of `fields` (`FOLD_KEYS`).
    folded: Option<Folded>,
}

struct Field {
    /// Shared with `Object::index`: one allocation per distinct key.
    name: Rc<str>,
    seen: usize,
    schema: Schema,
}

/// A wide object's fields folded into their names (and whether some of the
/// merged objects lack each) and one description of every value.
struct Folded {
    names: Vec<(Rc<str>, bool)>,
    values: Box<Schema>,
}

impl Object {
    fn slot(&mut self, name: &str) -> usize {
        // Looked up before allocating: most keys repeat across elements.
        if let Some(&at) = self.index.get(name) {
            return at;
        }
        let name: Rc<str> = Rc::from(name);
        self.fields.push(Field {
            name: Rc::clone(&name),
            seen: 0,
            schema: Schema::default(),
        });
        let at = self.fields.len() - 1;
        self.index.insert(name, at);
        at
    }

    /// Never had a key.
    fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.folded.is_none()
    }

    /// Lists its keys one per line.
    fn is_record(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Folds `other` in as if its objects had been added here. Runs before
    /// `collapse`, so neither side is `folded` yet.
    fn absorb(&mut self, other: Object) {
        self.seen += other.seen;
        for field in other.fields {
            let at = self.slot(&field.name);
            self.fields[at].seen += field.seen;
            self.fields[at].schema.absorb(field.schema);
        }
    }

    /// Whether the object values under this object's keys (or the objects
    /// in its array values) mostly share their keys: on average, a key of
    /// the merged value is present in at least half of the values. A table
    /// of records is (one odd extra field does not break it); a set of
    /// config sections, each with its own keys, is not — merged, nobody
    /// could tell `logging.level` from `db.level`.
    fn values_share_keys(&self) -> bool {
        let mut union = std::collections::HashSet::new();
        let mut occurrences = 0;
        let mut values = 0;
        for field in &self.fields {
            if let Some(object) = field.schema.value_object() {
                values += object.seen;
                for inner in &object.fields {
                    union.insert(&*inner.name);
                    occurrences += inner.seen;
                }
            }
        }
        2 * occurrences >= union.len() * values
    }

    /// Folds (`FOLD_KEYS`) when `may_fold`, then collapses whatever is
    /// listed under it.
    fn collapse(&mut self, may_fold: bool) {
        // A value that is only ever null says nothing about the kind.
        let mut kinds = self
            .fields
            .iter()
            .map(Field::kind)
            .filter(|&(bits, _)| bits != 0);
        let first = kinds.next();
        if may_fold
            && self.fields.len() > FOLD_KEYS
            && kinds.all(|kind| Some(kind) == first)
            && self.values_share_keys()
        {
            let seen = self.seen;
            let mut names = Vec::with_capacity(self.fields.len());
            let mut values = Schema::default();
            for field in std::mem::take(&mut self.fields) {
                names.push((field.name, field.seen < seen));
                values.absorb(field.schema);
            }
            self.index = Default::default();
            values.collapse();
            self.folded = Some(Folded {
                names,
                values: Box::new(values),
            });
            return;
        }
        for field in &mut self.fields {
            field.schema.collapse();
        }
    }
}

impl Field {
    /// What the fold compares: the kinds, with the strings' shared format.
    fn kind(&self) -> (u8, Option<Option<&'static str>>) {
        (self.schema.kind_bits(), self.schema.string)
    }
}

/// `a, "b c"?, d` in whole keys up to `FOLD_LIST_BYTES`, then `… N more`.
/// The first key always shows, cut with `…` when it alone is too long.
fn key_list(names: &[(Rc<str>, bool)]) -> String {
    let mut out = String::new();
    for (i, (name, optional)) in names.iter().enumerate() {
        let mut label = key_label(name);
        if *optional {
            label.push('?');
        }
        if i == 0 {
            if label.len() > FOLD_LIST_BYTES {
                let mut end = FOLD_LIST_BYTES;
                while !label.is_char_boundary(end) {
                    end -= 1;
                }
                out.push_str(&label[..end]);
                out.push('…');
            } else {
                out.push_str(&label);
            }
            continue;
        }
        if out.len() + 2 + label.len() > FOLD_LIST_BYTES {
            out.push_str(&format!(", … {} more", names.len() - i));
            break;
        }
        out.push_str(", ");
        out.push_str(&label);
    }
    out
}

impl Schema {
    fn add(&mut self, value: &Value) {
        match value {
            Value::Null => self.null = true,
            Value::Bool(_) => self.boolean = true,
            Value::Number(_) => self.number = true,
            Value::String(s) => self.note_string(string_tag(s)),
            Value::Array(items) => {
                let elements = self.array.get_or_insert_default();
                for item in items {
                    elements.add(item);
                }
            }
            Value::Object(map) => {
                let object = self.object.get_or_insert_default();
                object.seen += 1;
                for (key, value) in map {
                    let at = object.slot(key);
                    let field = &mut object.fields[at];
                    field.seen += 1;
                    field.schema.add(value);
                }
            }
        }
    }

    /// Folds `other` in as if its values had been added here.
    fn absorb(&mut self, other: Schema) {
        self.null |= other.null;
        self.boolean |= other.boolean;
        self.number |= other.number;
        if let Some(tag) = other.string {
            self.note_string(tag);
        }
        if let Some(object) = other.object {
            self.object.get_or_insert_default().absorb(object);
        }
        if let Some(elements) = other.array {
            self.array.get_or_insert_default().absorb(*elements);
        }
    }

    /// The object here, or the one the array elements here are.
    fn value_object(&self) -> Option<&Object> {
        self.object
            .as_ref()
            .or_else(|| self.array.as_deref().and_then(|a| a.object.as_ref()))
    }

    /// Strings of format `tag` were seen here: the format they all share,
    /// or `None` once two disagree.
    fn note_string(&mut self, tag: Option<&'static str>) {
        self.string = Some(match self.string {
            Some(seen) if seen != tag => None,
            _ => tag,
        });
    }

    /// Folds every wide object (`FOLD_KEYS`), all the way down. A key that
    /// is also an array keeps its object listed: folded values and array
    /// elements would both read as the `[]` line under it.
    fn collapse(&mut self) {
        let may_fold = self.array.is_none();
        if let Some(object) = &mut self.object {
            object.collapse(may_fold);
        }
        if let Some(elements) = &mut self.array {
            elements.collapse();
        }
    }

    /// Which kinds occur here, null aside.
    fn kind_bits(&self) -> u8 {
        u8::from(self.object.is_some())
            | u8::from(self.array.is_some()) << 1
            | u8::from(self.string.is_some()) << 2
            | u8::from(self.number) << 3
            | u8::from(self.boolean) << 4
    }

    fn is_empty(&self) -> bool {
        !self.has_scalar() && self.object.is_none() && self.array.is_none()
    }

    fn has_scalar(&self) -> bool {
        self.null || self.boolean || self.number || self.string.is_some()
    }

    fn is_only_object(&self) -> bool {
        self.object.is_some() && self.array.is_none() && !self.has_scalar()
    }

    fn is_only_array(&self) -> bool {
        self.array.is_some() && self.object.is_none() && !self.has_scalar()
    }

    /// Only ever `{}` or `[]`.
    fn is_only_empty_containers(&self) -> bool {
        !self.has_scalar()
            && self.object.as_ref().is_none_or(Object::is_empty)
            && self.array.as_deref().is_none_or(Schema::is_empty)
    }

    /// `object | array | string (url) | null`; a container that was always
    /// empty is spelled `{}` / `[]`, since nothing is listed under it, and a
    /// folded object `object, 2000 keys (AED, AFN, … 1998 more)`.
    fn kinds(&self) -> String {
        let mut kinds: Vec<String> = Vec::new();
        if let Some(object) = &self.object {
            kinds.push(match &object.folded {
                Some(folded) => format!(
                    "object, {} keys ({})",
                    folded.names.len(),
                    key_list(&folded.names)
                ),
                None if object.is_empty() => "{}".into(),
                None => "object".into(),
            });
        }
        if let Some(elements) = &self.array {
            kinds.push(if elements.is_empty() { "[]" } else { "array" }.into());
        }
        match self.string {
            Some(Some(tag)) => kinds.push(format!("string ({tag})")),
            Some(None) => kinds.push("string".into()),
            None => {}
        }
        if self.number {
            kinds.push("number".into());
        }
        if self.boolean {
            kinds.push("boolean".into());
        }
        if self.null {
            kinds.push("null".into());
        }
        kinds.join(" | ")
    }
}

#[derive(Clone)]
enum Seg {
    Key(String),
    Each,
}

/// The lines within the byte budget, built as the structure is walked: a
/// line past the budget, and every line after it, is only counted, so a
/// huge structure costs its kept lines, not its whole size.
struct Outline {
    budget: usize,
    /// What `lines` take, joined by `\n`.
    used: usize,
    lines: Vec<String>,
    /// Lines past the budget.
    dropped: usize,
    /// Every kept leaf's path, with the index of the line that shows it.
    leaves: Vec<(usize, Vec<Seg>)>,
}

impl Outline {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            lines: Vec::new(),
            dropped: 0,
            leaves: Vec::new(),
        }
    }

    fn line(&mut self, indent: usize, text: String) {
        if self.dropped > 0 {
            self.dropped += 1;
            return;
        }
        let line = format!("{}{text}", "  ".repeat(indent));
        let add = line.len() + usize::from(!self.lines.is_empty());
        if self.used + add > self.budget {
            self.dropped = 1;
            return;
        }
        self.used += add;
        self.lines.push(line);
    }

    /// Records the leaf the last line describes, if that line was kept.
    fn leaf(&mut self, path: &[Seg]) {
        if self.dropped == 0 && !path.is_empty() {
            self.leaves.push((self.lines.len() - 1, path.to_vec()));
        }
    }

    /// The kept lines, then (when some were dropped) a line counting the
    /// rest, together within the budget — only that count line alone may
    /// exceed a tiny one; and how many lines were kept.
    fn text(&mut self) -> (String, usize) {
        if self.dropped == 0 {
            return (self.lines.join("\n"), self.lines.len());
        }
        loop {
            let trailer = format!("… {} more lines not shown", self.dropped);
            let kept = self.lines.len();
            if kept == 0 || self.used + 1 + trailer.len() <= self.budget {
                let mut out = self.lines.join("\n");
                if kept > 0 {
                    out.push('\n');
                }
                out.push_str(&trailer);
                return (out, kept);
            }
            let line = self.lines.pop().expect("kept > 0");
            self.used -= line.len() + usize::from(!self.lines.is_empty());
            self.dropped += 1;
        }
    }

    /// One line per key, up to `LIST_KEYS`, then a line counting the rest.
    fn fields(&mut self, object: &Object, indent: usize, path: &mut Vec<Seg>) {
        for (i, field) in object.fields.iter().enumerate() {
            if i == LIST_KEYS {
                self.line(indent, format!("… {} more keys", object.fields.len() - i));
                return;
            }
            let mut label = key_label(&field.name);
            if field.seen < object.seen {
                label.push('?');
            }
            path.push(Seg::Key(field.name.to_string()));
            self.render(Some(label), &field.schema, indent, path);
            path.pop();
        }
    }

    /// `label` is `None` at the root: a root record lists its keys with no
    /// header line, and anything else at the root is labelled `.`.
    fn render(
        &mut self,
        label: Option<String>,
        schema: &Schema,
        indent: usize,
        path: &mut Vec<Seg>,
    ) {
        if let Some(object) = &schema.object
            && schema.is_only_object()
            && object.is_record()
        {
            match label {
                Some(label) => {
                    self.line(indent, label);
                    self.fields(object, indent + 1, path);
                }
                None => self.fields(object, indent, path),
            }
            return;
        }
        let label = label.unwrap_or_else(|| ".".into());
        if schema.is_only_array() {
            let elements = schema.array.as_deref().expect("only array");
            if elements.is_empty() {
                self.line(indent, format!("{label}: [] (always empty)"));
                return;
            }
            path.push(Seg::Each);
            self.render(Some(format!("{label}[]")), elements, indent, path);
            path.pop();
            return;
        }
        self.line(indent, format!("{label}: {}", schema.kinds()));
        let mut nested = false;
        if let Some(object) = &schema.object {
            if let Some(folded) = &object.folded {
                // The example reaches one value by its first key, the
                // way the model reaches a named one.
                path.push(Seg::Key(folded.names[0].0.to_string()));
                self.render(Some("[]".into()), &folded.values, indent + 1, path);
                path.pop();
                nested = true;
            } else if object.is_record() {
                self.fields(object, indent + 1, path);
                nested = true;
            }
        }
        if let Some(elements) = schema.array.as_deref()
            && !elements.is_empty()
        {
            path.push(Seg::Each);
            self.render(Some("[]".into()), elements, indent + 1, path);
            path.pop();
            nested = true;
        }
        // An always-empty `{}`/`[]` holds nothing to reach, so it never
        // becomes the example.
        if !nested && !schema.is_only_empty_containers() {
            self.leaf(path);
        }
    }
}

/// `.data[].address.city`, `.[]["odd key"]`.
fn jq_path(segs: &[Seg]) -> String {
    use crate::jq::path::{push_bracket, push_key};
    let mut out = String::new();
    for seg in segs {
        match seg {
            Seg::Key(k) => push_key(&mut out, k),
            Seg::Each => push_bracket(&mut out, ""),
        }
    }
    out
}

fn string_tag(s: &str) -> Option<&'static str> {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    if b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
    {
        return Some("date-time");
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return Some("url");
    }
    if b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
    {
        return Some("uuid");
    }
    if let Some((user, host)) = s.split_once('@')
        && !user.is_empty()
        && host.contains('.')
        && !host.contains(' ')
    {
        return Some("email");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(body: &str) -> String {
        shape(body, ShapeLimits::default()).unwrap().outline
    }

    #[test]
    fn scalars_become_type_names_and_keys_are_kept() {
        assert_eq!(
            outline(r#"{"id": 1, "name": "x", "ok": true, "none": null}"#),
            "id: number\nname: string\nok: boolean\nnone: null"
        );
    }

    #[test]
    fn nesting_is_indentation_with_no_depth_limit() {
        let body = r#"{"result": {"orders": [{"id": 1, "shipments": [{"packages": [{"items": [
            {"sku": "X1", "product": {"details": {"color": "red", "weight_kg": 1.2}}}
        ]}]}]}]}}"#;
        assert_eq!(
            outline(body),
            "\
result
  orders[]
    id: number
    shipments[]
      packages[]
        items[]
          sku: string
          product
            details
              color: string
              weight_kg: number"
        );
    }

    #[test]
    fn array_elements_are_merged_and_keys_missing_from_some_are_marked() {
        let body = r#"{"data": [
            {"id": 1, "manager": null},
            {"id": 2, "manager": "123e4567-e89b-12d3-a456-426614174000", "meta": {"code": "Z"}}
        ]}"#;
        assert_eq!(
            outline(body),
            "\
data[]
  id: number
  manager: string (uuid) | null
  meta?
    code: string"
        );
    }

    #[test]
    fn arrays_of_scalars_and_nested_arrays_stay_on_one_line() {
        assert_eq!(
            outline(r#"{"tags": ["a"], "grid": [[1, 2]], "none": [], "mixed": [1, "a", null]}"#),
            "tags[]: string\ngrid[][]: number\nnone: [] (always empty)\nmixed[]: string | number | null"
        );
    }

    #[test]
    fn a_value_that_is_sometimes_an_object_lists_its_keys_under_the_union() {
        assert_eq!(
            outline(r#"[{"a": {"b": 1}}, {"a": null}, {"a": [true]}]"#),
            "\
.[]
  a: object | array | null
    b: number
    []: boolean"
        );
    }

    #[test]
    fn a_root_array_or_scalar_is_named_by_its_jq_path() {
        assert_eq!(outline(r#"[{"id": 1}]"#), ".[]\n  id: number");
        assert_eq!(outline("[1, 2]"), ".[]: number");
        assert_eq!(outline("3"), ".: number");
    }

    #[test]
    fn keys_that_are_not_identifiers_are_quoted() {
        assert_eq!(
            outline(r#"{"first name": "x", "a.b": 1}"#),
            "\"first name\": string\n\"a.b\": number"
        );
    }

    #[test]
    fn recognisable_string_formats_are_tagged_only_when_every_value_agrees() {
        assert_eq!(
            outline(
                r#"{"t": "2026-09-03T10:00:00Z", "u": "https://x.test/a", "e": "a@b.co", "id": "123e4567-e89b-12d3-a456-426614174000"}"#
            ),
            "t: string (date-time)\nu: string (url)\ne: string (email)\nid: string (uuid)"
        );
        assert_eq!(
            outline(r#"[{"t": "2026-09-03"}, {"t": "soon"}]"#),
            ".[]\n  t: string"
        );
    }

    #[test]
    fn the_example_path_is_the_deepest_leaf_written_as_jq() {
        let s = shape(
            r#"{"top": 1, "data": [{"address": {"geo": {"lat": 1}}, "odd key": {"x": {"y": 1}}}]}"#,
            ShapeLimits::default(),
        )
        .unwrap();
        assert_eq!(s.example_path.as_deref(), Some(".data[].address.geo.lat"));
        let s = shape(r#"[{"odd key": [{"v": 1}]}]"#, ShapeLimits::default()).unwrap();
        assert_eq!(s.example_path.as_deref(), Some(r#".[]["odd key"][].v"#));
    }

    #[test]
    fn output_past_the_byte_cap_is_cut_at_a_line_and_says_how_much_is_missing() {
        let body = r#"{"alpha": 1, "beta": 2, "gamma": 3, "delta": 4}"#;
        // The four lines take 54 bytes; one short of that cuts.
        let s = shape(body, ShapeLimits { max_bytes: 53 }).unwrap();
        assert_eq!(
            s.outline,
            "alpha: number\nbeta: number\n… 2 more lines not shown"
        );
        assert_eq!(s.outline.len(), 53);
        // The count line is inside the cap too: a line is given up for it.
        let s = shape(body, ShapeLimits { max_bytes: 40 }).unwrap();
        assert_eq!(s.outline, "alpha: number\n… 3 more lines not shown");
        assert_eq!(s.outline.len(), 40);
    }

    #[test]
    fn a_first_line_past_the_cap_leaves_only_the_count_with_no_blank_line() {
        let s = shape(r#"{"alpha": 1, "beta": 2}"#, ShapeLimits { max_bytes: 5 }).unwrap();
        assert_eq!(s.outline, "… 2 more lines not shown");
    }

    /// `{"K00": <value>, "K01": <value>, …}` with `n` keys.
    fn keyed(n: usize, value: &str) -> String {
        let pairs: Vec<String> = (0..n).map(|i| format!(r#""K{i:02}": {value}"#)).collect();
        format!("{{{}}}", pairs.join(", "))
    }

    /// `K00, K01, …` for the first `n` keys `keyed` makes.
    fn key_names(n: usize) -> String {
        (0..n).map(|i| format!("K{i:02}")).collect::<Vec<_>>().join(", ")
    }

    #[test]
    fn a_wide_object_is_one_line_naming_its_keys_so_the_keys_after_it_still_show() {
        let body = format!(
            r#"{{"rates": {}, "data": [{{"id": 1}}]}}"#,
            keyed(FOLD_KEYS + 1, "1.5")
        );
        assert_eq!(
            outline(&body),
            format!(
                "rates: object, 21 keys ({})\n  []: number\ndata[]\n  id: number",
                key_names(21)
            )
        );
    }

    #[test]
    fn a_wide_record_keeps_every_field_name() {
        let names = [
            "views", "likes", "shares", "saves", "clicks", "opens", "replies", "reposts",
            "follows", "mentions", "quotes", "bookmarks", "impressions", "reach", "visits",
            "downloads", "installs", "signups", "logins", "purchases", "refunds", "votes",
        ];
        let pairs: Vec<String> = names.iter().map(|n| format!(r#""{n}": 1"#)).collect();
        let body = format!(r#"{{"stats": {{{}}}}}"#, pairs.join(", "));
        let s = shape(&body, ShapeLimits::default()).unwrap();
        assert_eq!(
            s.outline,
            format!("stats: object, 22 keys ({})\n  []: number", names.join(", "))
        );
        // The example reaches one value by name, as the model will.
        assert_eq!(s.example_path.as_deref(), Some(".stats.views"));
    }

    #[test]
    fn a_folded_object_merges_its_values_and_null_does_not_stop_the_fold() {
        let mut body = keyed(FOLD_KEYS, r#"{"name": "x"}"#);
        body.insert_str(
            body.len() - 1,
            r#", "late": {"name": "y", "extra": 1}, "gone": null"#,
        );
        assert_eq!(
            outline(&body),
            format!(
                ".: object, 22 keys ({}, late, gone)\n  []: object | null\n    name: string\n    extra?: number",
                key_names(FOLD_KEYS)
            )
        );
    }

    #[test]
    fn element_keys_that_are_ids_fold_into_one_line_of_quoted_keys() {
        let id = |i: usize| format!("123e4567-e89b-12d3-a456-4266141740{i:02}");
        let elements: Vec<String> = (0..FOLD_KEYS + 5)
            .map(|i| format!(r#"{{"{}": {{"n": {i}}}}}"#, id(i)))
            .collect();
        let s = shape(&format!("[{}]", elements.join(",")), ShapeLimits::default()).unwrap();
        // Each element has one of the keys, so every key is marked `?`. A
        // quoted id is 39 bytes with it and each after the first 41: nine
        // fit in 400.
        let shown: Vec<String> = (0..9).map(|i| format!("\"{}\"?", id(i))).collect();
        assert_eq!(
            s.outline,
            format!(
                ".[]: object, 25 keys ({}, … 16 more)\n  []\n    n: number",
                shown.join(", ")
            )
        );
        assert_eq!(
            s.example_path,
            Some(format!(r#".[]["{}"].n"#, id(0)))
        );
    }

    #[test]
    fn a_long_key_list_is_cut_at_a_whole_key_and_counts_the_rest() {
        // K00 is 3 bytes and each next key 5 (`, K01`): 80 keys make 398.
        assert_eq!(
            outline(&keyed(300, "1")),
            format!(".: object, 300 keys ({}, … 220 more)\n  []: number", key_names(80))
        );
    }

    #[test]
    fn a_first_key_longer_than_the_list_is_cut() {
        let key = "9".repeat(FOLD_LIST_BYTES + 50);
        let mut body = keyed(FOLD_KEYS, "true");
        body.insert_str(1, &format!(r#""{key}": false, "#));
        let label = format!("\"{key}\"");
        assert_eq!(
            outline(&body),
            format!(
                ".: object, 21 keys ({}…, … 20 more)\n  []: boolean",
                &label[..FOLD_LIST_BYTES]
            )
        );
    }

    #[test]
    fn sections_with_their_own_keys_stay_listed_so_each_field_keeps_its_owner() {
        // 21 config sections, all objects, no two sharing a key.
        let sections: Vec<String> = (0..FOLD_KEYS + 1)
            .map(|i| format!(r#""s{i:02}": {{"a{i:02}": 1, "b{i:02}": "x"}}"#))
            .collect();
        let lines = outline(&format!("{{{}}}", sections.join(", ")));
        assert!(!lines.contains("keys ("), "{lines}");
        assert!(lines.contains("s07\n  a07: number\n  b07: string\n"), "{lines}");
    }

    #[test]
    fn the_example_path_names_a_leaf_the_cut_outline_still_shows() {
        // A wide first record (mixed kinds, so it is not folded) fills the
        // budget; the deepest leaf is after it.
        let body = format!(
            r#"{{"wide": {{{}}}, "z": {{"a": {{"b": {{"c": 1}}}}}}}}"#,
            (0..60)
                .map(|i| format!(
                    r#""key_number_{i:02}": {}"#,
                    if i % 2 == 0 { "\"v\"" } else { "1" }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let s = shape(&body, ShapeLimits { max_bytes: 600 }).unwrap();
        assert!(s.outline.ends_with("more lines not shown"), "{}", s.outline);
        assert!(!s.outline.contains("\nz\n"), "{}", s.outline);
        assert_eq!(s.example_path.as_deref(), Some(".wide.key_number_00"));
        let s = shape(&body, ShapeLimits::default()).unwrap();
        assert_eq!(s.example_path.as_deref(), Some(".z.a.b.c"));
    }

    #[test]
    fn a_fold_keeps_which_keys_are_missing_and_needs_one_string_format() {
        // Three records; `nickname` is in one, `email` in all.
        let row = |extra: &str| {
            let mut pairs: Vec<String> = (0..FOLD_KEYS)
                .map(|i| format!(r#""c{i:02}": "v""#))
                .collect();
            pairs.push(r#""email": "a@b.co""#.into());
            if !extra.is_empty() {
                pairs.push(extra.into());
            }
            format!("{{{}}}", pairs.join(", "))
        };
        let body = format!("[{}, {}, {}]", row(""), row(""), row(r#""nickname": "n""#));
        let lines = outline(&body);
        // `email` is a formatted string, the columns are plain: no fold.
        assert!(!lines.contains("keys ("), "{lines}");
        assert!(lines.contains("\n  email: string (email)\n"), "{lines}");
        assert!(lines.contains("\n  nickname?: string"), "{lines}");
        // Plain strings throughout: folded, and the missing key is marked.
        let body = body.replace("a@b.co", "plain");
        let lines = outline(&body);
        assert!(lines.contains(", email, nickname?)\n  []: string"), "{lines}");
    }

    #[test]
    fn a_key_that_is_also_an_array_keeps_its_wide_object_listed() {
        // Folded values and the array's elements would both be a `[]` line.
        let body = format!(r#"[{{"a": {}}}, {{"a": ["x"]}}]"#, keyed(FOLD_KEYS + 1, "1"));
        let lines = outline(&body);
        assert!(lines.starts_with(".[]\n  a: object | array\n    K00: number\n"), "{lines}");
        assert_eq!(lines.matches("[]: string").count(), 1, "{lines}");
        assert!(!lines.contains("keys ("), "{lines}");
        // An always-empty array beside it is the same `[]` ambiguity.
        let body = format!(r#"[{{"a": {}}}, {{"a": []}}]"#, keyed(FOLD_KEYS + 1, "1"));
        let lines = outline(&body);
        assert!(lines.starts_with(".[]\n  a: object | []\n    K00: number\n"), "{lines}");
        assert!(!lines.contains("keys ("), "{lines}");
    }

    #[test]
    fn sections_of_arrays_with_their_own_record_types_stay_listed() {
        let sections: Vec<String> = (0..FOLD_KEYS + 1)
            .map(|i| format!(r#""s{i:02}": [{{"a{i:02}": 1}}]"#))
            .collect();
        let lines = outline(&format!("{{{}}}", sections.join(", ")));
        assert!(!lines.contains("keys ("), "{lines}");
        assert!(lines.contains("s07[]\n  a07: number\n"), "{lines}");
    }

    #[test]
    fn a_wide_object_of_mixed_kinds_lists_a_hundred_keys_then_counts_the_rest() {
        // Mixed kinds, so the fold passes it over; the keys after it must
        // still reach the model.
        let pairs: Vec<String> = (0..LIST_KEYS + 50)
            .map(|i| format!(r#""k{i:03}": {}"#, if i % 2 == 0 { "1" } else { "\"x\"" }))
            .collect();
        let body = format!(r#"{{"prices": {{{}}}, "total": 7}}"#, pairs.join(", "));
        let lines = outline(&body);
        assert_eq!(lines.lines().count(), 1 + LIST_KEYS + 1 + 1, "{lines}");
        assert!(lines.contains("  k099: string\n  … 50 more keys\ntotal: number"), "{lines}");
    }

    #[test]
    fn a_record_stays_listed_key_by_key() {
        // Exactly FOLD_KEYS same-kind keys is still a record.
        let lines = outline(&keyed(FOLD_KEYS, "1"));
        assert_eq!(lines.lines().count(), FOLD_KEYS, "{lines}");
        // Many keys of mixed kinds is a record, however many.
        let mut body = keyed(FOLD_KEYS + 10, "1");
        body.insert_str(body.len() - 1, r#", "name": "x""#);
        let lines = outline(&body);
        assert_eq!(lines.lines().count(), FOLD_KEYS + 11, "{lines}");
        assert!(!lines.contains("keys ("), "{lines}");
    }

    #[test]
    fn the_example_path_never_ends_at_an_always_empty_container() {
        let s = shape(
            r#"{"data": [{"id": 1, "meta": {"tags": {}, "refs": {"list": []}}}]}"#,
            ShapeLimits::default(),
        )
        .unwrap();
        assert_eq!(s.example_path.as_deref(), Some(".data[].id"));
        let s = shape(r#"{"a": {"b": []}}"#, ShapeLimits::default()).unwrap();
        assert_eq!(s.example_path, None);
    }

    #[test]
    fn non_json_has_no_shape() {
        assert_eq!(shape("nope", ShapeLimits::default()), None);
    }
}
