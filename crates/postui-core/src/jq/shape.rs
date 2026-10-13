//! A structure-only outline of a JSON body for the AI prompt: key names
//! and types, never values.

use serde_json::Value;

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
/// array is `name[]`. An object keyed by data rather than field names (see
/// `MAP_KEYS`) is one `map` line with its values merged under `[]`. No
/// depth limit; only the total size is capped, and a cut says how many
/// lines it dropped. Non-JSON → `None`.
pub fn shape(body: &str, limits: ShapeLimits) -> Option<Shape> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut schema = Schema::default();
    schema.add(&value);
    schema.collapse();
    let mut outline = Outline::default();
    let mut path = Vec::new();
    match &schema.object {
        Some(object) if schema.is_only_object() && object.is_record() => {
            outline.fields(object, 0, &mut path)
        }
        _ => outline.render(".".into(), &schema, 0, &mut path),
    }
    Some(Shape {
        outline: cap(&outline.lines, limits.max_bytes),
        example_path: outline.deepest.map(|segs| jq_path(&segs)),
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
/// kinds (null aside), is a lookup table keyed by data — currency codes,
/// dates, ids — not a record: listing each key would bury everything after
/// it under thousands of lines.
const MAP_KEYS: usize = 20;

/// The longest example key a `map` line shows before cutting it.
const MAP_EXAMPLE_CHARS: usize = 40;

#[derive(Default)]
struct Object {
    /// How many objects were merged, to tell which keys some lack.
    seen: usize,
    fields: Vec<Field>,
    index: std::collections::HashMap<String, usize>,
    /// Set by `collapse` in place of `fields` for a lookup table.
    map: Option<KeyMap>,
}

struct Field {
    name: String,
    seen: usize,
    schema: Schema,
}

/// A lookup table's fields folded into one description of every value.
struct KeyMap {
    keys: usize,
    /// The first key, to show what the keys look like.
    example: String,
    values: Box<Schema>,
}

impl Object {
    fn slot(&mut self, name: &str) -> usize {
        *self.index.entry(name.to_string()).or_insert_with(|| {
            self.fields.push(Field {
                name: name.to_string(),
                seen: 0,
                schema: Schema::default(),
            });
            self.fields.len() - 1
        })
    }

    /// Never had a key.
    fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.map.is_none()
    }

    /// Lists its keys one per line.
    fn is_record(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Folds `other` in as if its objects had been added here. Runs before
    /// `collapse`, so neither side has a `map` yet.
    fn absorb(&mut self, other: Object) {
        self.seen += other.seen;
        for field in other.fields {
            let at = self.slot(&field.name);
            self.fields[at].seen += field.seen;
            self.fields[at].schema.absorb(field.schema);
        }
    }

    fn collapse(&mut self) {
        // A value that is only ever null says nothing about the kind.
        let mut kinds = self
            .fields
            .iter()
            .map(|f| f.schema.kind_bits())
            .filter(|&bits| bits != 0);
        let first = kinds.next();
        if self.fields.len() > MAP_KEYS && kinds.all(|bits| Some(bits) == first) {
            let keys = self.fields.len();
            let example = self.fields[0].name.clone();
            let mut values = Schema::default();
            for field in std::mem::take(&mut self.fields) {
                values.absorb(field.schema);
            }
            self.index = Default::default();
            values.collapse();
            self.map = Some(KeyMap {
                keys,
                example,
                values: Box::new(values),
            });
            return;
        }
        for field in &mut self.fields {
            field.schema.collapse();
        }
    }
}

impl Schema {
    fn add(&mut self, value: &Value) {
        match value {
            Value::Null => self.null = true,
            Value::Bool(_) => self.boolean = true,
            Value::Number(_) => self.number = true,
            Value::String(s) => {
                let tag = string_tag(s);
                self.string = Some(match self.string {
                    Some(seen) if seen != tag => None,
                    _ => tag,
                });
            }
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
            self.string = Some(match self.string {
                Some(seen) if seen != tag => None,
                _ => tag,
            });
        }
        if let Some(object) = other.object {
            self.object.get_or_insert_default().absorb(object);
        }
        if let Some(elements) = other.array {
            self.array.get_or_insert_default().absorb(*elements);
        }
    }

    /// Turns every lookup table (`MAP_KEYS`) into a `KeyMap`, all the way
    /// down.
    fn collapse(&mut self) {
        if let Some(object) = &mut self.object {
            object.collapse();
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

    /// `object | array | string (url) | null`; a container that was always
    /// empty is spelled `{}` / `[]`, since nothing is listed under it, and a
    /// lookup table `map, 2000 keys like "USD"`.
    fn kinds(&self) -> String {
        let mut kinds: Vec<String> = Vec::new();
        if let Some(object) = &self.object {
            kinds.push(match &object.map {
                Some(map) => {
                    let mut example: String = map.example.chars().take(MAP_EXAMPLE_CHARS).collect();
                    let cut = example.len() < map.example.len();
                    example = serde_json::to_string(&example).expect("strings always encode");
                    if cut {
                        example.push('…');
                    }
                    format!("map, {} keys like {example}", map.keys)
                }
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

#[derive(Default)]
struct Outline {
    lines: Vec<String>,
    /// The longest path to a leaf, first found on a tie.
    deepest: Option<Vec<Seg>>,
}

impl Outline {
    fn line(&mut self, indent: usize, text: String) {
        self.lines.push(format!("{}{text}", "  ".repeat(indent)));
    }

    fn leaf(&mut self, path: &[Seg]) {
        if !path.is_empty() && self.deepest.as_ref().is_none_or(|d| path.len() > d.len()) {
            self.deepest = Some(path.to_vec());
        }
    }

    fn fields(&mut self, object: &Object, indent: usize, path: &mut Vec<Seg>) {
        for field in &object.fields {
            let mut label = if crate::jq::path::is_identifier(&field.name) {
                field.name.clone()
            } else {
                serde_json::to_string(&field.name).expect("strings always encode")
            };
            if field.seen < object.seen {
                label.push('?');
            }
            path.push(Seg::Key(field.name.clone()));
            self.render(label, &field.schema, indent, path);
            path.pop();
        }
    }

    fn render(&mut self, label: String, schema: &Schema, indent: usize, path: &mut Vec<Seg>) {
        if schema.is_only_array() {
            let elements = schema.array.as_deref().expect("only array");
            if elements.is_empty() {
                self.line(indent, format!("{label}: [] (always empty)"));
                self.leaf(path);
                return;
            }
            path.push(Seg::Each);
            self.render(format!("{label}[]"), elements, indent, path);
            path.pop();
            return;
        }
        if let Some(object) = &schema.object
            && schema.is_only_object()
            && object.is_record()
        {
            self.line(indent, label);
            self.fields(object, indent + 1, path);
            return;
        }
        self.line(indent, format!("{label}: {}", schema.kinds()));
        let mut nested = false;
        if let Some(object) = &schema.object {
            if let Some(map) = &object.map {
                path.push(Seg::Each);
                self.render("[]".into(), &map.values, indent + 1, path);
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
            self.render("[]".into(), elements, indent + 1, path);
            path.pop();
            nested = true;
        }
        if !nested {
            self.leaf(path);
        }
    }
}

/// `.data[].address.city`, `.[]["odd key"]`.
fn jq_path(segs: &[Seg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            Seg::Key(k) if crate::jq::path::is_identifier(k) => {
                out.push('.');
                out.push_str(k);
            }
            Seg::Key(k) => {
                if out.is_empty() {
                    out.push('.');
                }
                out.push('[');
                out.push_str(&serde_json::to_string(k).expect("strings always encode"));
                out.push(']');
            }
            Seg::Each => {
                if out.is_empty() {
                    out.push('.');
                }
                out.push_str("[]");
            }
        }
    }
    out
}

/// Whole lines up to `max_bytes`, then a line counting the rest.
fn cap(lines: &[String], max_bytes: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + line.len() > max_bytes {
            if sep == 1 {
                out.push('\n');
            }
            out.push_str(&format!("… {} more lines not shown", lines.len() - i));
            return out;
        }
        if sep == 1 {
            out.push('\n');
        }
        out.push_str(line);
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
        let s = shape(
            r#"{"alpha": 1, "beta": 2, "gamma": 3, "delta": 4}"#,
            ShapeLimits { max_bytes: 30 },
        )
        .unwrap();
        assert_eq!(
            s.outline,
            "alpha: number\nbeta: number\n… 2 more lines not shown"
        );
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

    #[test]
    fn a_lookup_table_is_one_map_line_so_the_keys_after_it_still_show() {
        let body = format!(
            r#"{{"rates": {}, "data": [{{"id": 1}}]}}"#,
            keyed(MAP_KEYS + 1, "1.5")
        );
        assert_eq!(
            outline(&body),
            "rates: map, 21 keys like \"K00\"\n  []: number\ndata[]\n  id: number"
        );
    }

    #[test]
    fn a_map_merges_its_values_and_null_does_not_stop_the_fold() {
        let mut body = keyed(MAP_KEYS, r#"{"name": "x"}"#);
        body.insert_str(
            body.len() - 1,
            r#", "late": {"name": "y", "extra": 1}, "gone": null"#,
        );
        assert_eq!(
            outline(&body),
            ".: map, 22 keys like \"K00\"\n  []: object | null\n    name: string\n    extra?: number"
        );
    }

    #[test]
    fn element_keys_that_are_ids_fold_into_one_map() {
        let elements: Vec<String> = (0..MAP_KEYS + 5)
            .map(|i| format!(r#"{{"123e4567-e89b-12d3-a456-4266141740{i:02}": {{"n": {i}}}}}"#))
            .collect();
        let s = shape(&format!("[{}]", elements.join(",")), ShapeLimits::default()).unwrap();
        assert_eq!(
            s.outline,
            ".[]: map, 25 keys like \"123e4567-e89b-12d3-a456-426614174000\"\n  []\n    n: number"
        );
        assert_eq!(s.example_path.as_deref(), Some(".[][].n"));
    }

    #[test]
    fn a_record_stays_listed_key_by_key() {
        // Exactly MAP_KEYS same-kind keys is still a record.
        let lines = outline(&keyed(MAP_KEYS, "1"));
        assert_eq!(lines.lines().count(), MAP_KEYS, "{lines}");
        // Many keys of mixed kinds is a record, however many.
        let mut body = keyed(MAP_KEYS + 10, "1");
        body.insert_str(body.len() - 1, r#", "name": "x""#);
        let lines = outline(&body);
        assert_eq!(lines.lines().count(), MAP_KEYS + 11, "{lines}");
        assert!(!lines.contains("map"), "{lines}");
    }

    #[test]
    fn a_long_example_key_is_cut() {
        let key = "k".repeat(MAP_EXAMPLE_CHARS + 10);
        let mut body = keyed(MAP_KEYS, "true");
        body.insert_str(1, &format!(r#""{key}": false, "#));
        let shown = format!("\"{}\"…", "k".repeat(MAP_EXAMPLE_CHARS));
        assert_eq!(
            outline(&body),
            format!(".: map, 21 keys like {shown}\n  []: boolean")
        );
    }

    #[test]
    fn non_json_has_no_shape() {
        assert_eq!(shape("nope", ShapeLimits::default()), None);
    }
}
